use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::bail;
use serde_json::{Value, json};

use crate::config::Config;
use crate::eval_events;
use crate::mode::ExecutionMode;
use crate::planner::profile::{profile_complete_scaffold, profile_setup_scaffold_paths};
use crate::planner::verify::{
    RuntimeCommandConnector, RuntimeNormalizedCommand, RuntimeNormalizedCommandSegment,
    VerifyInstallCommandFamily, diagnose_verify_command,
    normalize_runtime_bash_command_for_boundary, normalize_verify_command,
};
use crate::provider_call::{self, ProviderCallScope};
use crate::providers::ChatClient;
use crate::state::{ConversationMessage, SessionSnapshot, ToolCall};
use crate::tools::args_recovery::recover_tool_arguments;
use crate::tools::bash::BashOutcomeKind;
use crate::tools::path_guard::{
    resolve_existing, resolve_optional_existing, validate_workspace_relative,
};
use crate::tools::registry::{
    ToolContext, ToolRegistry, missing_arg_name, recoverable_tool_error, tool_error_kind,
};
use crate::tui::status::UiStatus;
use crate::tui::{InteractionUi, NOOP_UI};

use super::build_verifier::{BuildVerifierLifecycleObservation, BuildVerifierStatus};
use super::compact::compact_if_needed;
use super::completion::{
    CompletionContract, format_verify_feedback_with_contract, target_implementation_files,
};
use super::dependency_setup::{self, NodeDependencySetupAuthority, NodeDependencySetupStatus};
use super::evidence::{RuntimeAcceptanceReport, verify_runtime_acceptance};
use super::import_scan::{format_missing_import_feedback, scan_relative_imports};
use super::prompt::{ToolPromptMode, build_request_messages};
use super::reachability::{
    RepairReachability, assess_repair_reachability, reachability_failure_kind,
    reachability_recovery_reason,
};
use super::repair_progress::{
    RepairProgressVerdict, VerificationSignature, classify_repair_progress,
};
use super::repair_target::{
    RepairFollowThrough, RepairTarget, classify_repair_follow_through, classify_repair_target,
};
use super::verifier_bootstrap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStopReason {
    AssistantFinal,
    RequiredArtifactsSatisfiedAfterTool,
    CompletionContractSatisfied,
    CompletionContractObservedIncomplete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSessionOutcome {
    pub final_text: String,
    pub stop_reason: RunStopReason,
    pub changed_paths: Vec<String>,
    pub iterations: usize,
    pub tool_calls: usize,
    pub missing_required_paths: Vec<String>,
    pub missing_capabilities: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub missing_obligations: Vec<String>,
    pub verify_attempts: usize,
    pub last_blocking_reason: Option<String>,
    pub last_provider_error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunSessionErrorContext {
    pub missing_capabilities: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub missing_obligations: Vec<String>,
    pub repair_target: Option<String>,
}

impl RunSessionErrorContext {
    fn from_runtime_acceptance(
        runtime_acceptance: &RuntimeAcceptanceReport,
        repair_target: RepairTarget,
    ) -> Self {
        Self {
            missing_capabilities: runtime_acceptance.missing_capabilities.clone(),
            missing_evidence: runtime_acceptance.missing_evidence.clone(),
            missing_obligations: runtime_acceptance.missing_obligations.clone(),
            repair_target: Some(repair_target.as_str().to_string()),
        }
    }

    fn from_repair_target(repair_target: RepairTarget) -> Self {
        Self {
            repair_target: Some(repair_target.as_str().to_string()),
            ..Self::default()
        }
    }

    fn is_empty(&self) -> bool {
        self.missing_capabilities.is_empty()
            && self.missing_evidence.is_empty()
            && self.missing_obligations.is_empty()
            && self.repair_target.is_none()
    }
}

#[derive(Debug, Clone)]
pub struct RunSessionError {
    pub message: String,
    pub context: RunSessionErrorContext,
}

impl RunSessionError {
    fn new(message: impl Into<String>, context: RunSessionErrorContext) -> Self {
        Self {
            message: message.into(),
            context,
        }
    }
}

impl std::fmt::Display for RunSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}

impl std::error::Error for RunSessionError {}

const ARTIFACT_NON_EDIT_STAGNATION_THRESHOLD: usize = 3;
const ARTIFACT_RECOVERY_ATTEMPT_LIMIT: usize = 3;
const READ_ONLY_STAGNATION_INTERVENTION_THRESHOLD: usize = 3;
const READ_ONLY_STAGNATION_COMPACT_THRESHOLD: usize = 5;
const VERIFY_REPAIR_NO_EDIT_LIMIT: usize = 1;
const RECOVERABLE_TOOL_ERROR_REPEAT_LIMIT: usize = 2;
const MALFORMED_NATIVE_TOOL_RETRY_LIMIT: usize = 2;
const DEFAULT_STEP_WALL_CLOCK_CAP: Duration = Duration::from_secs(15 * 60);
const COMMAND_TIMEOUT_STRATEGY_FEEDBACK_AT: usize = 2;
const COMMAND_TIMEOUT_LOOP_LIMIT: usize = 3;
const EMPTY_RESPONSE_RECOVERY_EXTRA_ITERATIONS: usize = 3;
const SETUP_SCAFFOLD_COMPLETION_REMAINING_THRESHOLD: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptArtifactExtraction {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionContractPathMerge {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionContractVerification {
    Enabled,
    DisabledDuringStep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContractEnforcement {
    Enforce,
    Observe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionNoToolPolicy {
    RequireWriteForActionPrompt,
    RequireToolOnlyIfNoToolSeen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunSessionScope {
    MinimalLoop,
    PlanRunStep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunSessionStepKind {
    Inspect,
    Setup,
    Implement,
    Verify,
    Report,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupScaffoldCompletionTrigger {
    BudgetLow,
    Exhausted,
}

impl SetupScaffoldCompletionTrigger {
    fn as_str(self) -> &'static str {
        match self {
            Self::BudgetLow => "budget_low",
            Self::Exhausted => "exhausted",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RunSessionOptions {
    pub prompt_artifact_extraction: PromptArtifactExtraction,
    pub completion_contract_path_merge: CompletionContractPathMerge,
    pub completion_contract_verification: CompletionContractVerification,
    pub contract_enforcement: ContractEnforcement,
    pub phase_scope: Option<String>,
    pub action_no_tool_policy: ActionNoToolPolicy,
    pub scope: RunSessionScope,
    pub step_kind: Option<RunSessionStepKind>,
    pub dependency_setup_authority: NodeDependencySetupAuthority,
    pub step_wall_clock_cap: Option<Duration>,
    pub path_fallback_candidates: Vec<String>,
    pub require_mutation_before_contract_short_circuit: bool,
}

impl Default for RunSessionOptions {
    fn default() -> Self {
        Self {
            prompt_artifact_extraction: PromptArtifactExtraction::Enabled,
            completion_contract_path_merge: CompletionContractPathMerge::Enabled,
            completion_contract_verification: CompletionContractVerification::Enabled,
            contract_enforcement: ContractEnforcement::Enforce,
            phase_scope: None,
            action_no_tool_policy: ActionNoToolPolicy::RequireWriteForActionPrompt,
            scope: RunSessionScope::MinimalLoop,
            step_kind: None,
            dependency_setup_authority: NodeDependencySetupAuthority::None,
            step_wall_clock_cap: None,
            path_fallback_candidates: Vec::new(),
            require_mutation_before_contract_short_circuit: false,
        }
    }
}

impl RunSessionOptions {
    pub(crate) fn plan_step(step_kind: RunSessionStepKind) -> Self {
        Self::plan_step_with_enforcement(step_kind, ContractEnforcement::Enforce, None)
    }

    pub(crate) fn final_acceptance_repair() -> Self {
        let mut options = Self::plan_step(RunSessionStepKind::Implement);
        options.require_mutation_before_contract_short_circuit = true;
        options
    }

    pub(crate) fn plan_step_with_enforcement(
        step_kind: RunSessionStepKind,
        enforcement: ContractEnforcement,
        phase_scope: Option<String>,
    ) -> Self {
        let completion_contract_enabled = step_kind == RunSessionStepKind::Implement;
        let contract_path_merge_enabled =
            completion_contract_enabled && enforcement == ContractEnforcement::Enforce;
        Self {
            prompt_artifact_extraction: PromptArtifactExtraction::Disabled,
            completion_contract_path_merge: if contract_path_merge_enabled {
                CompletionContractPathMerge::Enabled
            } else {
                CompletionContractPathMerge::Disabled
            },
            completion_contract_verification: if completion_contract_enabled {
                CompletionContractVerification::Enabled
            } else {
                CompletionContractVerification::DisabledDuringStep
            },
            contract_enforcement: enforcement,
            phase_scope,
            action_no_tool_policy: ActionNoToolPolicy::RequireToolOnlyIfNoToolSeen,
            scope: RunSessionScope::PlanRunStep,
            step_kind: Some(step_kind),
            dependency_setup_authority: NodeDependencySetupAuthority::None,
            step_wall_clock_cap: None,
            path_fallback_candidates: Vec::new(),
            require_mutation_before_contract_short_circuit: false,
        }
    }

    pub(crate) fn with_dependency_setup_authority(
        mut self,
        authority: NodeDependencySetupAuthority,
    ) -> Self {
        self.dependency_setup_authority = authority;
        self
    }

    pub(crate) fn with_path_fallback_candidates(mut self, candidates: Vec<String>) -> Self {
        self.path_fallback_candidates = candidates;
        self
    }

    fn contract_runtime_enabled(&self) -> bool {
        self.completion_contract_verification == CompletionContractVerification::Enabled
    }

    fn contract_path_merge_enabled(&self) -> bool {
        self.completion_contract_path_merge == CompletionContractPathMerge::Enabled
    }

    fn prompt_artifact_extraction_enabled(&self) -> bool {
        self.prompt_artifact_extraction == PromptArtifactExtraction::Enabled
    }

    fn requires_action_tool_feedback(
        &self,
        write_or_edit_seen: bool,
        tool_call_count: usize,
    ) -> bool {
        match self.action_no_tool_policy {
            ActionNoToolPolicy::RequireWriteForActionPrompt => !write_or_edit_seen,
            ActionNoToolPolicy::RequireToolOnlyIfNoToolSeen => tool_call_count == 0,
        }
    }

    fn allows_tool_only_step_completion(&self) -> bool {
        self.scope == RunSessionScope::PlanRunStep
            && matches!(
                self.step_kind,
                Some(
                    RunSessionStepKind::Inspect
                        | RunSessionStepKind::Setup
                        | RunSessionStepKind::Verify
                )
            )
    }

    fn contract_enforcement_label(&self) -> &'static str {
        self.contract_enforcement.as_str()
    }
}

fn provider_call_scope_for_options(
    options: &RunSessionOptions,
    pending_feedback: Option<&str>,
) -> ProviderCallScope {
    if pending_feedback.is_some() {
        return ProviderCallScope::Repair;
    }
    match options.scope {
        RunSessionScope::MinimalLoop | RunSessionScope::PlanRunStep => ProviderCallScope::Executor,
    }
}

fn step_wall_clock_cap(options: &RunSessionOptions) -> Duration {
    options
        .step_wall_clock_cap
        .or_else(step_wall_clock_cap_from_env)
        .unwrap_or(DEFAULT_STEP_WALL_CLOCK_CAP)
}

fn step_wall_clock_cap_from_env() -> Option<Duration> {
    let value = std::env::var("ANVIL_STEP_WALL_CLOCK_CAP_MS").ok()?;
    let millis = value.trim().parse::<u64>().ok()?;
    Some(Duration::from_millis(millis))
}

fn record_time_sink(sinks: &mut Vec<TimeSink>, sink: TimeSink) {
    if sink.duration_ms == 0 {
        return;
    }
    sinks.push(sink);
    sinks.sort_by(|a, b| b.duration_ms.cmp(&a.duration_ms));
    sinks.truncate(5);
}

fn dominant_time_sink_text(sinks: &[TimeSink]) -> String {
    sinks
        .first()
        .map(|sink| {
            format!(
                "{} `{}` took {} ms",
                sink.kind, sink.label, sink.duration_ms
            )
        })
        .unwrap_or_else(|| "no timed command/provider recorded".to_string())
}

fn command_timeout_similarity_key(message: &str) -> String {
    let command = extract_command_timeout_command(message).unwrap_or_else(|| message.to_string());
    let normalized = command
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if normalized.contains("ls -r") {
        return "ls -R".to_string();
    }
    normalized
        .split(['&', ';', '|'])
        .next()
        .unwrap_or(normalized.as_str())
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_command_timeout_command(message: &str) -> Option<String> {
    let rest = message.strip_prefix("command_timeout: ")?;
    Some(rest.lines().next()?.trim().to_string())
}

fn command_timeout_sink_label(message: &str) -> String {
    let command = extract_command_timeout_command(message).unwrap_or_else(|| "unknown".to_string());
    let lower = command.to_ascii_lowercase();
    if lower.contains("ls -r") {
        if lower.contains("node_modules") || !lower.contains("src/") {
            return "ls -R recurses the workspace and can traverse node_modules; list src/ or specific files instead".to_string();
        }
        return "recursive directory listing is the dominant sink; list the smallest relevant directory instead".to_string();
    }
    format!(
        "command timeout sink: {}",
        eval_events::body_snippet(&command)
    )
}

fn command_timeout_strategy_feedback(message: &str, repeats: usize) -> String {
    format!(
        "Bash command timed out repeatedly (similar timeout #{repeats}). {}. Change strategy now: avoid broad recursive listings, inspect targeted files/directories, or use Read/Glob/Grep for bounded inspection.",
        command_timeout_sink_label(message)
    )
}

fn continue_with_timeout_feedback(session: &mut SessionSnapshot, call: ToolCall, feedback: String) {
    session.messages.push(ConversationMessage::tool_result(
        call.name,
        Some(call.id),
        feedback,
    ));
}

impl ContractEnforcement {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ContractEnforcement::Enforce => "enforce",
            ContractEnforcement::Observe => "observe",
        }
    }
}

impl RunSessionScope {
    fn as_str(self) -> &'static str {
        match self {
            RunSessionScope::MinimalLoop => "minimal-loop",
            RunSessionScope::PlanRunStep => "plan-run-step",
        }
    }
}

impl RunSessionStepKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            RunSessionStepKind::Inspect => "inspect",
            RunSessionStepKind::Setup => "setup",
            RunSessionStepKind::Implement => "implement",
            RunSessionStepKind::Verify => "verify",
            RunSessionStepKind::Report => "report",
            RunSessionStepKind::Unknown => "unknown",
        }
    }

    fn bash_policy_purpose(self) -> &'static str {
        match self {
            RunSessionStepKind::Inspect => "runtime_inspection",
            RunSessionStepKind::Setup => "runtime_setup",
            RunSessionStepKind::Implement => "runtime_implementation",
            RunSessionStepKind::Verify | RunSessionStepKind::Report => {
                "deterministic_verifier_evidence"
            }
            RunSessionStepKind::Unknown => "runtime_unknown",
        }
    }

    fn requires_verifier_bash_policy(self) -> bool {
        matches!(
            self,
            RunSessionStepKind::Verify | RunSessionStepKind::Report
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeBashPolicyDecision {
    step_kind: &'static str,
    bash_policy_purpose: &'static str,
    verifier_policy_checked: bool,
    verifier_policy_ok: bool,
    deterministic_verifier_evidence: bool,
    blocked: bool,
    policy_error_kind: &'static str,
    violation_kind: &'static str,
    reason: String,
    normalized_command: Option<String>,
    split_segments: Vec<RuntimeNormalizedCommandSegment>,
    normalization_kind: &'static str,
    normalization_reason: String,
}

impl RuntimeBashPolicyDecision {
    fn for_step(step_kind: RunSessionStepKind, command: &str, root: &Path) -> Self {
        let verifier_policy_checked = step_kind.requires_verifier_bash_policy();
        if !verifier_policy_checked {
            return Self {
                step_kind: step_kind.as_str(),
                bash_policy_purpose: step_kind.bash_policy_purpose(),
                verifier_policy_checked: false,
                verifier_policy_ok: true,
                deterministic_verifier_evidence: false,
                blocked: false,
                policy_error_kind: "",
                violation_kind: "",
                reason: "runtime Bash is not deterministic verifier evidence".to_string(),
                normalized_command: None,
                split_segments: Vec::new(),
                normalization_kind: "",
                normalization_reason: String::new(),
            };
        }
        let normalized_plan = match normalize_runtime_bash_command_for_boundary(command, root) {
            Ok(plan) => plan,
            Err(err) => {
                let diagnosis = diagnose_verify_command(command);
                let violation = diagnosis
                    .violation
                    .unwrap_or(crate::planner::verify::VerifyCommandViolationKind::Blocked);
                let reason = diagnosis.reason.unwrap_or_else(|| err.to_string());
                return Self {
                    step_kind: step_kind.as_str(),
                    bash_policy_purpose: step_kind.bash_policy_purpose(),
                    verifier_policy_checked: true,
                    verifier_policy_ok: false,
                    deterministic_verifier_evidence: false,
                    blocked: true,
                    policy_error_kind: "verify_command_policy_error",
                    violation_kind: violation.as_str(),
                    reason,
                    normalized_command: None,
                    split_segments: Vec::new(),
                    normalization_kind: "",
                    normalization_reason: String::new(),
                };
            }
        };
        let normalized_command = (normalized_plan.normalized_command != command.trim())
            .then(|| normalized_plan.normalized_command.clone());
        let reason = if normalized_command.is_some() {
            "runtime Bash admitted as deterministic verifier evidence after mechanical normalization"
        } else {
            "runtime Bash admitted as deterministic verifier evidence"
        };
        Self {
            step_kind: step_kind.as_str(),
            bash_policy_purpose: step_kind.bash_policy_purpose(),
            verifier_policy_checked: true,
            verifier_policy_ok: true,
            deterministic_verifier_evidence: true,
            blocked: false,
            policy_error_kind: "",
            violation_kind: "",
            reason: reason.to_string(),
            normalized_command,
            split_segments: normalized_plan.segments,
            normalization_kind: normalized_plan.normalization_kind,
            normalization_reason: normalized_plan.normalization_reason,
        }
    }
}

fn runtime_bash_policy_decision(
    options: &RunSessionOptions,
    tool_name: &str,
    arguments: &Value,
    root: &Path,
) -> Option<RuntimeBashPolicyDecision> {
    if tool_name != "Bash" {
        return None;
    }
    let recovered = recover_tool_arguments(tool_name, arguments.clone());
    let command = recovered.arguments.get("command").and_then(Value::as_str)?;
    let step_kind = options.step_kind.unwrap_or(RunSessionStepKind::Unknown);
    Some(RuntimeBashPolicyDecision::for_step(
        step_kind, command, root,
    ))
}

fn recovered_bash_command(tool_name: &str, arguments: &Value) -> Option<String> {
    if tool_name != "Bash" {
        return None;
    }
    let recovered = recover_tool_arguments(tool_name, arguments.clone());
    recovered
        .arguments
        .get("command")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn set_bash_command(arguments: &mut Value, command: String) {
    if let Some(object) = arguments.as_object_mut() {
        object.insert("command".to_string(), json!(command));
    } else {
        *arguments = json!({ "command": command });
    }
}

fn deterministic_verify_substitute(root: &Path, required_paths: &[String]) -> Option<String> {
    let path = required_paths
        .iter()
        .find(|path| resolve_existing(root, path).is_err())
        .or_else(|| required_paths.first())?;
    crate::tools::path_guard::validate_workspace_relative(path).ok()?;
    let command = format!("test -f {path}");
    normalize_verify_command(&command)
        .ok()
        .map(|command| command.into_string())
}

fn emit_runtime_bash_policy(
    path: Option<&Path>,
    decision: &RuntimeBashPolicyDecision,
    command: &str,
) {
    eval_events::emit(
        path,
        json!({
            "event": "runtime_bash_policy",
            "tool_name": "Bash",
            "step_kind": decision.step_kind,
            "bash_policy_purpose": decision.bash_policy_purpose,
            "verifier_policy_checked": decision.verifier_policy_checked,
            "verifier_policy_ok": decision.verifier_policy_ok,
            "deterministic_verifier_evidence": decision.deterministic_verifier_evidence,
            "blocked": decision.blocked,
            "policy_error_kind": decision.policy_error_kind,
            "verify_command_violation_kind": decision.violation_kind,
            "normalization_kind": decision.normalization_kind,
            "normalized_command_summary": decision.normalized_command.as_deref().map(eval_events::body_snippet).unwrap_or_default(),
            "reason": eval_events::body_snippet(&decision.reason),
            "command_summary": eval_events::body_snippet(command),
        }),
    );
}

#[allow(clippy::too_many_arguments)]
fn execute_split_runtime_bash<F, G>(
    segments: &[RuntimeNormalizedCommandSegment],
    root: &Path,
    profile: &str,
    setup_authority: NodeDependencySetupAuthority,
    offline: bool,
    eval_events_path: Option<&Path>,
    is_cancelled: F,
    is_force_cancelled: G,
) -> anyhow::Result<String>
where
    F: Fn() -> bool,
    G: Fn() -> bool,
{
    let mut out = Vec::new();
    let mut and_chain_failed = false;
    let mut first_failure: Option<(usize, String, BashOutcomeKind)> = None;
    for (index, segment) in segments.iter().enumerate() {
        if segment.connector == RuntimeCommandConnector::Always {
            and_chain_failed = false;
        }
        if segment.connector == RuntimeCommandConnector::AndThen && and_chain_failed {
            out.push(format!(
                "segment {} skipped by && short-circuit: {}",
                index + 1,
                segment.command.as_str()
            ));
            continue;
        }
        let command = segment.command.as_str();
        match &segment.command {
            RuntimeNormalizedCommand::DependencyInstall { family, .. } => {
                let setup = run_runtime_verify_install_substitution(
                    root,
                    profile,
                    command,
                    *family,
                    setup_authority,
                    offline,
                    eval_events_path,
                );
                let passed = runtime_dependency_setup_allows_verify_continuation(&setup);
                out.push(format!(
                    "segment {} install substituted: {}\nsetup_status: {}\nfeedback: dependency installs are owned by the runtime; verify with the build/test command alone.",
                    index + 1,
                    command,
                    setup.status.as_str()
                ));
                if passed {
                    and_chain_failed = false;
                } else {
                    and_chain_failed = true;
                    if first_failure.is_none() {
                        first_failure = Some((
                            index + 1,
                            command.to_string(),
                            BashOutcomeKind::CommandFailed,
                        ));
                    }
                }
            }
            RuntimeNormalizedCommand::Verify(verify_command) => {
                let command = verify_command.as_str();
                let outcome = crate::tools::bash::run_structured_cancel_and_force(
                    command,
                    root,
                    offline,
                    Duration::from_secs(180),
                    &is_cancelled,
                    &is_force_cancelled,
                )?;
                match outcome.kind {
                    BashOutcomeKind::Blocked => bail!("{}", outcome.summary),
                    BashOutcomeKind::Timeout => bail!(
                        "command_timeout: {command}\n{}",
                        crate::tools::bash::format_outcome(&outcome)
                    ),
                    BashOutcomeKind::Cancelled => bail!(
                        "command_aborted_by_user: interrupted by user: {command}\n{}",
                        crate::tools::bash::format_outcome(&outcome)
                    ),
                    BashOutcomeKind::Success | BashOutcomeKind::CommandFailed => {}
                }
                out.push(format!(
                    "segment {} command: {}\n{}",
                    index + 1,
                    command,
                    crate::tools::bash::format_outcome(&outcome)
                ));
                if outcome.kind == BashOutcomeKind::Success {
                    and_chain_failed = false;
                } else {
                    and_chain_failed = true;
                    if first_failure.is_none() {
                        first_failure = Some((index + 1, command.to_string(), outcome.kind));
                    }
                }
            }
        }
    }
    let combined = if let Some((index, command, kind)) = first_failure {
        format!("combined_outcome: {kind:?}\nfailing_segment: {index} `{command}`")
    } else {
        "combined_outcome: Success".to_string()
    };
    Ok(format!("{combined}\n{}", out.join("\n\n")))
}

fn run_runtime_verify_install_substitution(
    root: &Path,
    profile: &str,
    command: &str,
    family: VerifyInstallCommandFamily,
    setup_authority: NodeDependencySetupAuthority,
    offline: bool,
    eval_events_path: Option<&Path>,
) -> dependency_setup::NodeDependencySetupObservation {
    let requirement = match family {
        VerifyInstallCommandFamily::Python => {
            dependency_setup::requirement_for_python_cli_dependencies(
                root,
                Some("python-cli"),
                "verify_segment dependency reconciliation",
                setup_authority,
            )
        }
        VerifyInstallCommandFamily::Node => {
            let canonical = profile.trim().to_ascii_lowercase();
            if canonical == "nextjs"
                && dependency_setup::package_json_declares_dependencies(root)
                && !dependency_setup::next_build_dependencies_ready(root)
            {
                dependency_setup::requirement_for_next_build(
                    root,
                    Some("nextjs"),
                    "verify_segment dependency reconciliation",
                    setup_authority,
                )
            } else {
                dependency_setup::requirement_for_node_declared_dependencies(
                    root,
                    Some(profile),
                    "verify_segment dependency reconciliation",
                    setup_authority,
                )
            }
        }
    };
    let setup = dependency_setup::run_node_dependency_setup_with_program_and_offline(
        root,
        &requirement,
        Path::new("npm"),
        offline,
    );
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "verify_install_substituted",
            "trigger": "verify_segment",
            "command": eval_events::body_snippet(command),
            "family": family.as_str(),
            "setup_kind": setup.setup_kind.as_str(),
            "setup_status": setup.status.as_str(),
            "setup_attempted": setup.attempted,
            "setup_authority": setup.authority.as_str(),
            "feedback": "dependency installs are owned by the runtime; verify with the build/test command alone.",
        }),
    );
    setup
}

fn runtime_dependency_setup_allows_verify_continuation(
    setup: &dependency_setup::NodeDependencySetupObservation,
) -> bool {
    matches!(
        setup.status,
        NodeDependencySetupStatus::Passed | NodeDependencySetupStatus::NotRequired
    ) || setup.primary_reason.contains("already present")
        || setup.primary_reason.contains("has no dependency table")
        || setup
            .primary_reason
            .contains("has no project.dependencies table")
}

#[derive(Debug, Default)]
struct VerifyRepairState {
    pending_signature: Option<VerificationSignature>,
    pending_target: Option<RepairTarget>,
    pending_error_context: RunSessionErrorContext,
    changed_paths_at_failure: Vec<String>,
    no_edit_turns: usize,
}

#[derive(Debug)]
struct VerifyFailureFeedback {
    feedback: String,
    signature: VerificationSignature,
    target: RepairTarget,
    error_context: RunSessionErrorContext,
}

#[derive(Debug, Clone, Default)]
struct ContractObservation {
    missing_paths: Vec<String>,
    missing_capabilities: Vec<String>,
    missing_evidence: Vec<String>,
    missing_obligations: Vec<String>,
    primary_reason: String,
}

impl ContractObservation {
    fn from_report(
        report: &crate::planner::verify::VerificationReport,
        runtime_acceptance: &RuntimeAcceptanceReport,
    ) -> Self {
        Self {
            missing_paths: report.missing_paths.clone(),
            missing_capabilities: runtime_acceptance.missing_capabilities.clone(),
            missing_evidence: runtime_acceptance.missing_evidence.clone(),
            missing_obligations: runtime_acceptance.missing_obligations.clone(),
            primary_reason: report.primary_reason(),
        }
    }
}

#[derive(Debug)]
enum ContractVerificationOutcome {
    Satisfied,
    NeedsRepair(VerifyFailureFeedback),
    ObservationIncomplete(ContractObservation),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepShortCircuitAt {
    Start,
    Iteration,
}

impl StepShortCircuitAt {
    fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Iteration => "iteration",
        }
    }
}

struct ShortCircuitContext<'a> {
    verify_attempts: &'a mut usize,
    at: StepShortCircuitAt,
    write_or_edit_seen: bool,
}

#[derive(Debug)]
enum VerifyRepairNoEditOutcome {
    NoPendingFailure,
    Feedback(String),
    ObservationIncomplete(ContractObservation),
}

#[derive(Debug, Default)]
struct ArtifactRecoveryState {
    target_path: Option<String>,
    target_attempts: usize,
    last_model_action: Option<String>,
}

impl ArtifactRecoveryState {
    fn sync_target(&mut self, required_paths: &[String], missing: &[String]) -> Option<String> {
        let next = required_paths
            .iter()
            .find(|path| missing.contains(path))
            .cloned()
            .or_else(|| missing.first().cloned());
        if self.target_path != next {
            self.target_path = next.clone();
            self.target_attempts = 0;
        }
        next
    }

    fn record_action(&mut self, action: &str) {
        self.last_model_action = Some(action.to_string());
    }
}

#[derive(Debug, Default)]
struct RecoverableToolErrorState {
    key: Option<String>,
    repeats: usize,
}

impl RecoverableToolErrorState {
    fn record(&mut self, tool_name: &str, err: &anyhow::Error) -> usize {
        let kind = tool_error_kind(err);
        let key = if kind == "command_timeout" {
            format!(
                "{tool_name}:{kind}:{}",
                command_timeout_similarity_key(&err.to_string())
            )
        } else {
            format!("{tool_name}:{kind}:{err}")
        };
        if self.key.as_deref() == Some(key.as_str()) {
            self.repeats += 1;
        } else {
            self.key = Some(key);
            self.repeats = 1;
        }
        self.repeats
    }

    fn reset(&mut self) {
        self.key = None;
        self.repeats = 0;
    }
}

#[derive(Debug, Clone, serde::Serialize)]
struct TimeSink {
    kind: &'static str,
    label: String,
    duration_ms: u128,
}

pub fn run_session(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    user_prompt: &str,
    config: &Config,
) -> anyhow::Result<String> {
    run_session_with_required_paths(client, session, user_prompt, &[], config)
}

pub fn run_session_with_required_paths(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    user_prompt: &str,
    required_paths: &[String],
    config: &Config,
) -> anyhow::Result<String> {
    run_session_with_required_paths_with_ui(
        client,
        session,
        user_prompt,
        required_paths,
        config,
        &NOOP_UI,
    )
}

pub fn run_session_with_required_paths_with_ui(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    user_prompt: &str,
    required_paths: &[String],
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<String> {
    Ok(
        run_session_with_outcome_with_ui(client, session, user_prompt, required_paths, config, ui)?
            .final_text,
    )
}

pub fn run_session_with_outcome_with_ui(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    user_prompt: &str,
    required_paths: &[String],
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<RunSessionOutcome> {
    run_session_with_outcome_with_options(
        client,
        session,
        user_prompt,
        required_paths,
        config,
        ui,
        RunSessionOptions::default(),
    )
}

pub(crate) fn run_session_with_outcome_with_options(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    user_prompt: &str,
    required_paths: &[String],
    config: &Config,
    ui: &dyn InteractionUi,
    options: RunSessionOptions,
) -> anyhow::Result<RunSessionOutcome> {
    let registry = ToolRegistry::default();
    let mut native_tools_enabled =
        client.supports_native_tools(&config.model) && !session.native_tools_disabled;
    let completion_contract =
        if options.contract_runtime_enabled() || options.contract_path_merge_enabled() {
            CompletionContract::load_for_config(config)?
        } else {
            None
        };
    let explicit_required_paths = !required_paths.is_empty();
    let path_sources = effective_required_path_sources(
        &config.workspace_root,
        required_paths,
        user_prompt,
        completion_contract
            .as_ref()
            .map(|contract| contract.required_paths.as_slice())
            .unwrap_or(&[]),
        &options,
    );
    let required_paths = path_sources.effective_required_paths.clone();
    let initially_missing_paths = missing_paths(&config.workspace_root, &required_paths);
    emit_step_obligation_scope(
        config.eval_events_path.as_deref(),
        &options,
        &path_sources,
        &initially_missing_paths,
    );
    let mut pending_feedback: Option<String> = None;
    let mut verify_attempts = 0usize;
    let mut last_blocking_reason: Option<String> = None;
    let last_provider_error: Option<String> = None;
    let mut write_or_edit_seen = false;
    let mut no_tool_feedbacks = 0usize;
    let mut empty_feedbacks = 0usize;
    let mut empty_fresh_retry_pending = false;
    let mut provider_turn_timeouts = 0usize;
    let mut changed_paths: Vec<String> = Vec::new();
    let mut tool_call_count = 0usize;
    let contract_runtime_enabled = options.contract_runtime_enabled();
    let artifact_recovery_enabled =
        explicit_required_paths || (contract_runtime_enabled && completion_contract.is_some());
    let mut artifact_non_edit_streak = 0usize;
    let mut read_only_streak = 0usize;
    let mut artifact_recovery_state = ArtifactRecoveryState::default();
    let mut verify_repair_state = VerifyRepairState::default();
    let mut recoverable_tool_error_state = RecoverableToolErrorState::default();
    let mut edit_anchor_failures_by_path: BTreeMap<String, usize> = BTreeMap::new();
    let mut malformed_native_tool_feedbacks = 0usize;
    let step_capability_gate = StepCapabilityGate::from_prompt(user_prompt, &options);
    let step_started = Instant::now();
    let step_wall_clock_cap = step_wall_clock_cap(&options);
    let mut time_sinks: Vec<TimeSink> = Vec::new();
    let iteration_limit = if contract_runtime_enabled && completion_contract.is_some() {
        config.max_iterations
            + ARTIFACT_RECOVERY_ATTEMPT_LIMIT.saturating_mul(required_paths.len().max(1))
            + 1
    } else {
        config.max_iterations
    }
    .saturating_add(EMPTY_RESPONSE_RECOVERY_EXTRA_ITERATIONS);
    if let Some(outcome) = maybe_short_circuit_satisfied_step(
        config,
        &options,
        user_prompt,
        &required_paths,
        completion_contract
            .as_ref()
            .filter(|_| contract_runtime_enabled),
        ShortCircuitContext {
            verify_attempts: &mut verify_attempts,
            at: StepShortCircuitAt::Start,
            write_or_edit_seen,
        },
    )? {
        return Ok(outcome);
    }
    session
        .messages
        .push(ConversationMessage::user(user_prompt.to_string()));
    let profile_guidance = crate::planner::profile::profile_guidance(&config.profile, user_prompt);

    for iteration in 0..iteration_limit {
        if iteration > 0
            && pending_feedback.is_none()
            && let Some(outcome) = maybe_short_circuit_satisfied_step(
                config,
                &options,
                user_prompt,
                &required_paths,
                completion_contract
                    .as_ref()
                    .filter(|_| contract_runtime_enabled),
                ShortCircuitContext {
                    verify_attempts: &mut verify_attempts,
                    at: StepShortCircuitAt::Iteration,
                    write_or_edit_seen,
                },
            )?
        {
            return Ok(outcome);
        }
        let iterations_used = iteration + 1;
        let remaining_iterations = iteration_limit.saturating_sub(iterations_used);
        if step_started.elapsed() >= step_wall_clock_cap {
            let dominant = dominant_time_sink_text(&time_sinks);
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "loop_stop",
                    "reason": "step_wall_clock_exhausted",
                    "elapsed_ms": step_started.elapsed().as_millis(),
                    "cap_ms": step_wall_clock_cap.as_millis(),
                    "dominant_time_sink": dominant,
                    "top_time_sinks": time_sinks,
                    "session_scope": options.scope.as_str(),
                    "step_kind": options.step_kind.map(RunSessionStepKind::as_str).unwrap_or(""),
                    "phase_scope": options.phase_scope.as_deref().unwrap_or(""),
                }),
            );
            bail!("step_wall_clock_exhausted: {dominant}");
        }
        if ui.interrupted() {
            bail!("interrupted by user");
        }
        compact_if_needed(&mut session.messages, config.context_budget);
        let specs = registry.specs().to_vec();
        let request_tools = if native_tools_enabled {
            specs.clone()
        } else {
            Vec::new()
        };
        let empty_fresh_retry_active = empty_fresh_retry_pending;
        let fresh_retry_messages;
        let request_messages = if empty_fresh_retry_active {
            empty_fresh_retry_pending = false;
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "empty_response_escalation",
                    "stage": "fresh_session_retry",
                    "attempt": empty_feedbacks + 1,
                    "session_scope": options.scope.as_str(),
                    "step_kind": options.step_kind.map(RunSessionStepKind::as_str).unwrap_or(""),
                    "phase_scope": options.phase_scope.as_deref().unwrap_or(""),
                }),
            );
            fresh_retry_messages = vec![ConversationMessage::user(user_prompt.to_string())];
            build_request_messages(
                &fresh_retry_messages,
                &specs,
                &config.workspace_root,
                None,
                profile_guidance.as_deref(),
                if native_tools_enabled {
                    ToolPromptMode::Native
                } else {
                    ToolPromptMode::XmlFallback
                },
            )
        } else {
            build_request_messages(
                &session.messages,
                &specs,
                &config.workspace_root,
                pending_feedback.as_deref(),
                profile_guidance.as_deref(),
                if native_tools_enabled {
                    ToolPromptMode::Native
                } else {
                    ToolPromptMode::XmlFallback
                },
            )
        };
        let label = format!("{} {}", client.label(), config.model);
        let call_scope = provider_call_scope_for_options(&options, pending_feedback.as_deref());
        let chat_outcome = {
            let _guard = ui.before_model_call(&label);
            provider_call::chat_with_cancel(
                client,
                config,
                provider_call::ProviderChatRequest {
                    scope: call_scope,
                    model: &config.model,
                    messages: &request_messages,
                    tools: &request_tools,
                    native_tools_enabled,
                },
                || ui.interrupted(),
            )
        };
        let provider_turn_elapsed = chat_outcome.elapsed;
        record_time_sink(
            &mut time_sinks,
            TimeSink {
                kind: "provider",
                label: call_scope.as_str().to_string(),
                duration_ms: provider_turn_elapsed.as_millis(),
            },
        );
        let provider_turn_timed_out = chat_outcome.timed_out
            || provider_turn_elapsed >= Duration::from_secs(config.chat_timeout_secs);
        let chat_result = chat_outcome.result;
        if provider_turn_timed_out {
            provider_turn_timeouts += 1;
            let terminal = provider_turn_timeouts > 1;
            emit_provider_turn_timeout(config, &options, provider_turn_timeouts, terminal);
            if terminal {
                return stop_for_provider_turn_timeout(
                    config,
                    user_prompt,
                    &options,
                    &changed_paths,
                    verify_attempts,
                    tool_call_count,
                    last_blocking_reason,
                    provider_turn_timeouts,
                    provider_turn_elapsed,
                );
            }
            pending_feedback = Some(super::feedback::provider_turn_timeout(
                config.chat_timeout_secs,
            ));
            continue;
        } else if provider_turn_timeouts > 0 {
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "provider_turn_timeout_recovered",
                    "after_timeouts": provider_turn_timeouts,
                    "session_scope": options.scope.as_str(),
                    "step_kind": options.step_kind.map(RunSessionStepKind::as_str).unwrap_or(""),
                    "phase_scope": options.phase_scope.as_deref().unwrap_or(""),
                }),
            );
            provider_turn_timeouts = 0;
        }
        let reply = match chat_result {
            Ok(reply) => {
                pending_feedback = None;
                malformed_native_tool_feedbacks = 0;
                reply
            }
            Err(err)
                if native_tools_enabled
                    && provider_error_allows_native_tool_retry(&err)
                    && malformed_native_tool_feedbacks < MALFORMED_NATIVE_TOOL_RETRY_LIMIT =>
            {
                malformed_native_tool_feedbacks += 1;
                eval_events::emit(
                    config.eval_events_path.as_deref(),
                    json!({
                        "event": "native_tool_parse_retry_feedback",
                        "attempt": malformed_native_tool_feedbacks,
                        "attempt_limit": MALFORMED_NATIVE_TOOL_RETRY_LIMIT,
                        "reason": eval_events::body_snippet(&err.to_string()),
                    }),
                );
                pending_feedback = Some(super::feedback::malformed_tool_call(&err.to_string()));
                continue;
            }
            Err(err)
                if native_tools_enabled
                    && client.allows_xml_fallback()
                    && provider_error_allows_xml_fallback(&err) =>
            {
                eval_events::emit(
                    config.eval_events_path.as_deref(),
                    json!({
                        "event": "fallback_decision",
                        "from": "native_tools",
                        "to": "xml_fallback",
                        "allowed": true,
                        "reason": eval_events::body_snippet(&err.to_string()),
                    }),
                );
                native_tools_enabled = false;
                session.native_tools_disabled = true;
                pending_feedback = Some(super::feedback::malformed_tool_call(&err.to_string()));
                continue;
            }
            Err(err) => {
                let message = err.to_string();
                if native_tools_enabled && client.allows_xml_fallback() {
                    eval_events::emit(
                        config.eval_events_path.as_deref(),
                        json!({
                            "event": "fallback_decision",
                            "from": "native_tools",
                            "to": "xml_fallback",
                            "allowed": false,
                            "reason": eval_events::body_snippet(&message),
                        }),
                    );
                }
                return Err(err);
            }
        };
        ui.publish_status(UiStatus::for_model_reply(
            config,
            &config.model,
            client.label(),
            reply.prompt_tokens,
            reply.completion_tokens,
        ));
        if ui.interrupted() {
            bail!("interrupted by user");
        }
        let mut tool_calls = Vec::new();
        for mut call in reply.tool_calls.clone() {
            let raw_shape = eval_events::argument_shape(&call.arguments);
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "tool_call_raw",
                    "name": call.name.as_str(),
                    "arguments": raw_shape,
                }),
            );
            let recovered = recover_tool_arguments(&call.name, call.arguments.clone());
            if recovered.changed {
                eval_events::emit(
                    config.eval_events_path.as_deref(),
                    json!({
                        "event": "tool_args_recovered",
                        "name": call.name.as_str(),
                        "changes": recovered.changes,
                        "arguments": eval_events::argument_shape(&recovered.arguments),
                    }),
                );
                call.arguments = recovered.arguments;
            }
            tool_calls.push(call);
        }
        tool_call_count += tool_calls.len();
        let empty_reply_without_tools = tool_calls.is_empty() && reply.content.trim().is_empty();
        if !empty_reply_without_tools && (empty_feedbacks > 0 || empty_fresh_retry_active) {
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "empty_response_recovered",
                    "after_empty_responses": empty_feedbacks,
                    "fresh_session_retry": empty_fresh_retry_active,
                    "session_scope": options.scope.as_str(),
                    "step_kind": options.step_kind.map(RunSessionStepKind::as_str).unwrap_or(""),
                    "phase_scope": options.phase_scope.as_deref().unwrap_or(""),
                }),
            );
            empty_feedbacks = 0;
            empty_fresh_retry_pending = false;
        }
        session.messages.push(ConversationMessage::assistant(
            reply.content.clone(),
            tool_calls.clone(),
        ));
        if tool_calls.is_empty() {
            let mut missing = missing_paths(&config.workspace_root, &required_paths);
            if !missing.is_empty() {
                if remaining_iterations <= SETUP_SCAFFOLD_COMPLETION_REMAINING_THRESHOLD {
                    maybe_complete_setup_scaffold(
                        config,
                        &options,
                        user_prompt,
                        &missing,
                        SetupScaffoldCompletionTrigger::BudgetLow,
                        &mut changed_paths,
                    )?;
                    missing = missing_paths(&config.workspace_root, &required_paths);
                    if missing.is_empty() {
                        session.messages.pop();
                        return Ok(stop_for_required_artifacts_satisfied(
                            config,
                            &required_paths,
                            RequiredArtifactsStop {
                                changed_paths,
                                iterations: iterations_used,
                                tool_calls: tool_call_count,
                                verify_attempts,
                                last_blocking_reason,
                                last_provider_error,
                            },
                        ));
                    }
                }
                session.messages.pop();
                artifact_non_edit_streak =
                    artifact_non_edit_streak.saturating_add(ARTIFACT_NON_EDIT_STAGNATION_THRESHOLD);
                artifact_recovery_state.record_action("no_tool_missing_artifacts");
                if maybe_rescue_artifact_recovery_exhaustion(
                    config,
                    &options,
                    user_prompt,
                    ArtifactRecoveryRescueInput {
                        state: &artifact_recovery_state,
                        non_edit_streak: artifact_non_edit_streak,
                        enabled: artifact_recovery_enabled,
                        missing_paths: &missing,
                    },
                    &mut changed_paths,
                )? {
                    missing = missing_paths(&config.workspace_root, &required_paths);
                    if missing.is_empty() {
                        return Ok(stop_for_required_artifacts_satisfied(
                            config,
                            &required_paths,
                            RequiredArtifactsStop {
                                changed_paths,
                                iterations: iterations_used,
                                tool_calls: tool_call_count,
                                verify_attempts,
                                last_blocking_reason,
                                last_provider_error,
                            },
                        ));
                    }
                }
                if let Some(feedback) = maybe_artifact_recovery_feedback(
                    &mut artifact_recovery_state,
                    &mut artifact_non_edit_streak,
                    ArtifactRecoveryFeedbackContext {
                        eval_events_path: config.eval_events_path.as_deref(),
                        enabled: artifact_recovery_enabled,
                        missing_paths: &missing,
                        required_paths: &required_paths,
                        contract: completion_contract
                            .as_ref()
                            .filter(|_| contract_runtime_enabled),
                        root: &config.workspace_root,
                    },
                )? {
                    last_blocking_reason = Some("artifact creation stalled".to_string());
                    pending_feedback = Some(feedback);
                    continue;
                }
                last_blocking_reason =
                    Some(format!("missing required paths: {}", missing.join(", ")));
                pending_feedback = Some(super::feedback::missing_artifacts(&missing));
                continue;
            }
            if reply.content.trim().is_empty() {
                session.messages.pop();
                last_blocking_reason = Some("empty assistant response".to_string());
                if empty_fresh_retry_active {
                    return stop_for_model_empty_response(
                        config,
                        user_prompt,
                        &options,
                        &changed_paths,
                        verify_attempts,
                        tool_call_count,
                        last_blocking_reason,
                        last_provider_error,
                        empty_feedbacks + 1,
                    );
                }
                empty_feedbacks += 1;
                match empty_feedbacks {
                    1 => {
                        emit_empty_response_escalation(
                            config,
                            &options,
                            "nudge_1",
                            empty_feedbacks,
                        );
                        pending_feedback = Some(super::feedback::empty_response());
                        continue;
                    }
                    2 => {
                        emit_empty_response_escalation(
                            config,
                            &options,
                            "nudge_2",
                            empty_feedbacks,
                        );
                        pending_feedback =
                            Some(super::feedback::empty_response_reformulated(user_prompt));
                        continue;
                    }
                    3 => {
                        emit_empty_response_escalation(
                            config,
                            &options,
                            "fresh_session_retry_scheduled",
                            empty_feedbacks,
                        );
                        pending_feedback = None;
                        empty_fresh_retry_pending = true;
                        continue;
                    }
                    _ => {
                        return stop_for_model_empty_response(
                            config,
                            user_prompt,
                            &options,
                            &changed_paths,
                            verify_attempts,
                            tool_call_count,
                            last_blocking_reason,
                            last_provider_error,
                            empty_feedbacks,
                        );
                    }
                }
            }
            if options.requires_action_tool_feedback(write_or_edit_seen, tool_call_count)
                && looks_like_action_prompt(user_prompt)
            {
                if no_tool_feedbacks < 1 {
                    no_tool_feedbacks += 1;
                    session.messages.pop();
                    last_blocking_reason = Some("completion without write".to_string());
                    pending_feedback = Some(super::feedback::completion_without_write());
                    continue;
                }
                session.messages.pop();
                bail!("missing tool call for action prompt after feedback");
            }
            if options.requires_action_tool_feedback(write_or_edit_seen, tool_call_count)
                && looks_like_progress_without_tool(&reply.content)
                && no_tool_feedbacks < 3
            {
                no_tool_feedbacks += 1;
                session.messages.pop();
                last_blocking_reason = Some("progress text without tool call".to_string());
                pending_feedback = Some(super::feedback::no_tool_progress());
                continue;
            }
            let mut import_scan_paths = changed_paths.clone();
            import_scan_paths.extend(required_paths.iter().cloned());
            let missing_imports =
                scan_relative_imports(&config.workspace_root, &import_scan_paths)?;
            if !missing_imports.is_empty() {
                session.messages.pop();
                last_blocking_reason = Some("missing relative imports".to_string());
                pending_feedback = Some(format_missing_import_feedback(&missing_imports));
                continue;
            }
            if let Some(contract) = completion_contract
                .as_ref()
                .filter(|_| contract_runtime_enabled)
                .filter(|contract| contract.has_verify())
            {
                match handle_verify_repair_no_edit(
                    &config.workspace_root,
                    config.eval_events_path.as_deref(),
                    contract,
                    user_prompt,
                    &mut verify_repair_state,
                    &changed_paths,
                    &options,
                )? {
                    VerifyRepairNoEditOutcome::NoPendingFailure => {}
                    VerifyRepairNoEditOutcome::Feedback(feedback) => {
                        session.messages.pop();
                        last_blocking_reason = Some("verify repair missing edit".to_string());
                        pending_feedback = Some(feedback);
                        continue;
                    }
                    VerifyRepairNoEditOutcome::ObservationIncomplete(observation) => {
                        return Ok(RunSessionOutcome {
                            final_text: reply.content,
                            stop_reason: RunStopReason::CompletionContractObservedIncomplete,
                            changed_paths,
                            iterations: iteration + 1,
                            tool_calls: tool_call_count,
                            missing_required_paths: observation.missing_paths,
                            missing_capabilities: observation.missing_capabilities,
                            missing_evidence: observation.missing_evidence,
                            missing_obligations: observation.missing_obligations,
                            verify_attempts,
                            last_blocking_reason,
                            last_provider_error,
                        });
                    }
                }
                match verify_completion_contract_with_enforcement(
                    &config.workspace_root,
                    config.eval_events_path.as_deref(),
                    contract,
                    user_prompt,
                    &mut verify_attempts,
                    verify_repair_state.pending_signature.as_ref(),
                    verify_repair_state.pending_target,
                    &verify_repair_state.changed_paths_at_failure,
                    &changed_paths,
                    &[],
                    true,
                    options.dependency_setup_authority,
                    config.offline,
                    &options,
                ) {
                    Ok(ContractVerificationOutcome::Satisfied) => {
                        eval_events::emit(
                            config.eval_events_path.as_deref(),
                            json!({
                                "event": "loop_stop",
                                "reason": "completion_contract_satisfied",
                                "required_paths": required_paths,
                                "verify_attempts": verify_attempts,
                            }),
                        );
                        return Ok(RunSessionOutcome {
                            final_text: reply.content,
                            stop_reason: RunStopReason::CompletionContractSatisfied,
                            changed_paths,
                            iterations: iteration + 1,
                            tool_calls: tool_call_count,
                            missing_required_paths: Vec::new(),
                            missing_capabilities: Vec::new(),
                            missing_evidence: Vec::new(),
                            missing_obligations: Vec::new(),
                            verify_attempts,
                            last_blocking_reason,
                            last_provider_error,
                        });
                    }
                    Ok(ContractVerificationOutcome::NeedsRepair(feedback)) => {
                        session.messages.pop();
                        last_blocking_reason = Some("completion verify failed".to_string());
                        verify_repair_state.pending_error_context = feedback.error_context.clone();
                        verify_repair_state.pending_signature = Some(feedback.signature);
                        verify_repair_state.pending_target = Some(feedback.target);
                        verify_repair_state.changed_paths_at_failure = changed_paths.clone();
                        verify_repair_state.no_edit_turns = 0;
                        pending_feedback = Some(feedback.feedback);
                        continue;
                    }
                    Ok(ContractVerificationOutcome::ObservationIncomplete(observation)) => {
                        return Ok(RunSessionOutcome {
                            final_text: reply.content,
                            stop_reason: RunStopReason::CompletionContractObservedIncomplete,
                            changed_paths,
                            iterations: iteration + 1,
                            tool_calls: tool_call_count,
                            missing_required_paths: observation.missing_paths,
                            missing_capabilities: observation.missing_capabilities,
                            missing_evidence: observation.missing_evidence,
                            missing_obligations: observation.missing_obligations,
                            verify_attempts,
                            last_blocking_reason,
                            last_provider_error,
                        });
                    }
                    Err(err) => {
                        return Err(err);
                    }
                }
            }
            return Ok(RunSessionOutcome {
                final_text: reply.content,
                stop_reason: RunStopReason::AssistantFinal,
                changed_paths,
                iterations: iteration + 1,
                tool_calls: tool_call_count,
                missing_required_paths: Vec::new(),
                missing_capabilities: Vec::new(),
                missing_evidence: Vec::new(),
                missing_obligations: Vec::new(),
                verify_attempts,
                last_blocking_reason,
                last_provider_error,
            });
        }

        let context = ToolContext {
            root: config.workspace_root.clone(),
            mode: ExecutionMode::Act,
            auto_approve: config.yes,
            interactive_approval: false,
            offline: config.offline,
            workspace_policy: crate::tools::workspace_policy::WorkspacePolicy::for_task_request(),
            eval_events_path: config.eval_events_path.clone(),
            expected_paths: tool_context_expected_paths(&required_paths, &options),
        };
        let mut names_seen = BTreeSet::new();
        let mut batch_had_edit = false;
        let mut batch_changed_paths = Vec::new();
        let mut batch_non_edit_tools = 0usize;
        let mut batch_all_read_only_tools = !tool_calls.is_empty();
        let mut batch_had_recoverable_tool_error = false;
        let missing_before_batch = missing_paths(&config.workspace_root, &required_paths);
        for mut call in tool_calls {
            let mut split_bash_segments = Vec::new();
            if ui.interrupted() {
                bail!("interrupted by user");
            }
            let call_is_edit = matches!(call.name.as_str(), "Write" | "Edit");
            if !matches!(call.name.as_str(), "Read" | "Glob" | "Grep") {
                batch_all_read_only_tools = false;
            }
            if call_is_edit {
                batch_had_edit = true;
            } else {
                batch_non_edit_tools += 1;
            }
            if !names_seen.insert(call.name.clone()) {
                // Multiple same-tool calls are fine; this keeps clippy from seeing unused state.
            }
            if let (Some(command), Some(decision)) = (
                recovered_bash_command(&call.name, &call.arguments),
                runtime_bash_policy_decision(
                    &options,
                    &call.name,
                    &call.arguments,
                    &config.workspace_root,
                ),
            ) {
                emit_runtime_bash_policy(config.eval_events_path.as_deref(), &decision, &command);
                if decision.blocked {
                    batch_had_recoverable_tool_error = true;
                    let policy_error =
                        anyhow::anyhow!("{}: {}", decision.policy_error_kind, decision.reason);
                    let repeats = recoverable_tool_error_state.record(&call.name, &policy_error);
                    eval_events::emit(
                        config.eval_events_path.as_deref(),
                        json!({
                            "event": "tool_policy_error",
                            "name": call.name.as_str(),
                            "policy_error_kind": decision.policy_error_kind,
                            "verify_command_violation_kind": decision.violation_kind,
                            "bash_policy_purpose": decision.bash_policy_purpose,
                            "step_kind": decision.step_kind,
                            "deterministic_verifier_evidence": false,
                            "repeat_count": repeats,
                        }),
                    );
                    if decision.policy_error_kind == "verify_command_policy_error"
                        && repeats >= RECOVERABLE_TOOL_ERROR_REPEAT_LIMIT
                        && let Some(substitute) =
                            deterministic_verify_substitute(&config.workspace_root, &required_paths)
                    {
                        eval_events::emit(
                            config.eval_events_path.as_deref(),
                            json!({
                                "event": "verify_command_substituted",
                                "original": eval_events::body_snippet(&command),
                                "substitute": &substitute,
                                "reason": "policy_repetition",
                                "repeat_count": repeats,
                                "step_kind": decision.step_kind,
                                "oracle_tier": "degraded",
                                "degradation": "deterministic_expected_path_substitution",
                            }),
                        );
                        eval_events::emit(
                            config.eval_events_path.as_deref(),
                            json!({
                                "event": "step_oracle_tier_degraded",
                                "reason": "policy_repetition",
                                "oracle_tier": "degraded",
                                "original": eval_events::body_snippet(&command),
                                "substitute": &substitute,
                                "step_kind": decision.step_kind,
                            }),
                        );
                        set_bash_command(&mut call.arguments, substitute);
                        recoverable_tool_error_state.reset();
                    } else {
                        eval_events::emit(
                            config.eval_events_path.as_deref(),
                            json!({
                                "event": "tool_validation_error",
                                "name": call.name.as_str(),
                                "error_kind": decision.policy_error_kind,
                                "missing_arg": null,
                                "repeat_count": repeats,
                            }),
                        );
                        if repeats > RECOVERABLE_TOOL_ERROR_REPEAT_LIMIT {
                            eval_events::emit(
                                config.eval_events_path.as_deref(),
                                json!({
                                    "event": "loop_stop",
                                    "reason": "recoverable_tool_error_repeated",
                                    "name": call.name.as_str(),
                                    "error_kind": decision.policy_error_kind,
                                    "repeat_count": repeats - 1,
                                }),
                            );
                            bail!(
                                "recoverable tool error repeated: {}",
                                decision.policy_error_kind
                            );
                        }
                        let feedback = recoverable_tool_feedback(&call.name, &policy_error, None);
                        session.messages.push(ConversationMessage::tool_result(
                            call.name,
                            Some(call.id),
                            feedback,
                        ));
                        continue;
                    }
                }
                if let Some(normalized_command) = decision.normalized_command.clone() {
                    eval_events::emit(
                        config.eval_events_path.as_deref(),
                        json!({
                            "event": "verify_command_normalized_at_runtime",
                            "classification": "deterministic_verify_policy",
                            "kind": decision.normalization_kind,
                            "normalization_source": decision.normalization_kind,
                            "original": eval_events::body_snippet(&command),
                            "repaired": eval_events::body_snippet(&normalized_command),
                            "reason": eval_events::body_snippet(&decision.normalization_reason),
                        }),
                    );
                    set_bash_command(&mut call.arguments, normalized_command);
                }
                split_bash_segments = decision.split_segments.clone();
            }
            let result = {
                let _guard = ui.before_tool_call(&call.name);
                let split_has_install = split_bash_segments
                    .iter()
                    .any(|segment| segment.command.install_family().is_some());
                if call.name == "Bash" && (split_bash_segments.len() > 1 || split_has_install) {
                    execute_split_runtime_bash(
                        &split_bash_segments,
                        &context.root,
                        &config.profile,
                        options.dependency_setup_authority,
                        context.offline,
                        config.eval_events_path.as_deref(),
                        || ui.interrupted(),
                        || ui.force_interrupted(),
                    )
                } else {
                    registry.execute_with_cancel(
                        &call.name,
                        &call.arguments,
                        &context,
                        || ui.interrupted(),
                        || ui.force_interrupted(),
                    )
                }
            };
            let result = match result {
                Ok(result) => {
                    recoverable_tool_error_state.reset();
                    eval_events::emit(
                        config.eval_events_path.as_deref(),
                        json!({
                            "event": "tool_execute",
                            "name": call.name.as_str(),
                            "status": "ok",
                        }),
                    );
                    if matches!(call.name.as_str(), "Write" | "Edit") {
                        write_or_edit_seen = true;
                        read_only_streak = 0;
                        if let Some(path) =
                            changed_path_from_call(&config.workspace_root, &call.arguments)
                        {
                            if !changed_paths.contains(&path) {
                                changed_paths.push(path.clone());
                            }
                            if !batch_changed_paths.contains(&path) {
                                batch_changed_paths.push(path);
                            }
                        }
                    }
                    result
                }
                Err(err) if recoverable_tool_error(&err) => {
                    batch_had_recoverable_tool_error = true;
                    let kind = tool_error_kind(&err);
                    let err_text = err.to_string();
                    let repeats = recoverable_tool_error_state.record(&call.name, &err);
                    let duration_ms = if kind == "command_timeout" {
                        extract_elapsed_ms(&err_text)
                    } else {
                        None
                    };
                    if let Some(duration_ms) = duration_ms {
                        record_time_sink(
                            &mut time_sinks,
                            TimeSink {
                                kind: "command",
                                label: extract_command_timeout_command(&err_text)
                                    .unwrap_or_else(|| call.name.clone()),
                                duration_ms: u128::from(duration_ms),
                            },
                        );
                    }
                    eval_events::emit(
                        config.eval_events_path.as_deref(),
                        json!({
                            "event": "tool_validation_error",
                            "name": call.name.as_str(),
                            "error_kind": kind,
                            "missing_arg": missing_arg_name(&err),
                            "repeat_count": repeats,
                            "duration_ms": duration_ms,
                        }),
                    );
                    if kind == "command_timeout" {
                        let sink = command_timeout_sink_label(&err_text);
                        eval_events::emit(
                            config.eval_events_path.as_deref(),
                            json!({
                                "event": "command_timeout_repetition",
                                "name": call.name.as_str(),
                                "repeat_count": repeats,
                                "similarity_key": command_timeout_similarity_key(&err_text),
                                "sink": sink,
                                "duration_ms": duration_ms,
                                "strategy_feedback": repeats == COMMAND_TIMEOUT_STRATEGY_FEEDBACK_AT,
                                "terminal": repeats >= COMMAND_TIMEOUT_LOOP_LIMIT,
                            }),
                        );
                        if repeats >= COMMAND_TIMEOUT_LOOP_LIMIT {
                            eval_events::emit(
                                config.eval_events_path.as_deref(),
                                json!({
                                    "event": "loop_stop",
                                    "reason": "command_timeout_loop",
                                    "name": call.name.as_str(),
                                    "error_kind": kind,
                                    "repeat_count": repeats,
                                    "sink": sink,
                                    "top_time_sinks": time_sinks,
                                }),
                            );
                            bail!("command_timeout_loop: {sink}");
                        }
                        if repeats == COMMAND_TIMEOUT_STRATEGY_FEEDBACK_AT {
                            continue_with_timeout_feedback(
                                session,
                                call,
                                command_timeout_strategy_feedback(&err_text, repeats),
                            );
                            continue;
                        }
                    }
                    if repeats > RECOVERABLE_TOOL_ERROR_REPEAT_LIMIT {
                        eval_events::emit(
                            config.eval_events_path.as_deref(),
                            json!({
                                "event": "loop_stop",
                                "reason": "recoverable_tool_error_repeated",
                                "name": call.name.as_str(),
                                "error_kind": kind,
                                "repeat_count": repeats - 1,
                            }),
                        );
                        bail!("recoverable tool error repeated: {kind}");
                    }
                    let edit_anchor_failure = if kind == "edit_anchor_not_found" {
                        edit_anchor_tracking_path(&config.workspace_root, &call.arguments).map(
                            |path| {
                                let count = edit_anchor_failures_by_path
                                    .entry(path.clone())
                                    .or_default();
                                *count += 1;
                                (path, *count)
                            },
                        )
                    } else {
                        None
                    };
                    recoverable_tool_feedback(&call.name, &err, edit_anchor_failure)
                }
                Err(err) => {
                    eval_events::emit(
                        config.eval_events_path.as_deref(),
                        json!({
                            "event": "tool_execute",
                            "name": call.name.as_str(),
                            "status": "error",
                            "error_kind": tool_error_kind(&err),
                        }),
                    );
                    return Err(err);
                }
            };
            if call.name == "Bash"
                && let Some(duration_ms) = extract_elapsed_ms(&result)
            {
                record_time_sink(
                    &mut time_sinks,
                    TimeSink {
                        kind: "command",
                        label: recovered_bash_command(&call.name, &call.arguments)
                            .unwrap_or_else(|| "Bash".to_string()),
                        duration_ms: u128::from(duration_ms),
                    },
                );
            }
            session.messages.push(ConversationMessage::tool_result(
                call.name,
                Some(call.id),
                result,
            ));
        }
        let missing_after_batch = missing_paths(&config.workspace_root, &required_paths);
        let batch_reduced_missing_paths = missing_after_batch.len() < missing_before_batch.len();
        if batch_reduced_missing_paths {
            artifact_non_edit_streak = 0;
            read_only_streak = 0;
            artifact_recovery_state.record_action("required_artifact_progress");
        } else {
            artifact_non_edit_streak += if batch_non_edit_tools > 0 {
                batch_non_edit_tools
            } else if batch_had_edit {
                1
            } else {
                0
            };
            if batch_had_edit {
                artifact_recovery_state.record_action("edit_without_required_artifact_progress");
            } else if batch_non_edit_tools > 0 {
                artifact_recovery_state.record_action("non_edit_tool");
            }
        }
        if implement_step(&options)
            && batch_all_read_only_tools
            && !batch_had_recoverable_tool_error
        {
            read_only_streak = read_only_streak.saturating_add(1);
            if let Some(feedback) = maybe_read_only_stagnation_feedback(
                config.eval_events_path.as_deref(),
                user_prompt,
                read_only_streak,
                &options,
            ) {
                last_blocking_reason = Some("model_stagnation:read_only_loop".to_string());
                pending_feedback = Some(feedback);
                continue;
            }
        } else if !batch_all_read_only_tools {
            read_only_streak = 0;
        }
        if required_paths.is_empty()
            && options.allows_tool_only_step_completion()
            && !batch_had_recoverable_tool_error
        {
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "loop_stop",
                    "reason": "step_tool_observation_completed",
                    "tool_calls": tool_call_count,
                }),
            );
            return Ok(RunSessionOutcome {
                final_text: "step tool observation completed".to_string(),
                stop_reason: RunStopReason::AssistantFinal,
                changed_paths,
                iterations: iteration + 1,
                tool_calls: tool_call_count,
                missing_required_paths: Vec::new(),
                missing_capabilities: Vec::new(),
                missing_evidence: Vec::new(),
                missing_obligations: Vec::new(),
                verify_attempts,
                last_blocking_reason,
                last_provider_error,
            });
        }
        if required_paths_satisfied_after_tool(
            &config.workspace_root,
            &required_paths,
            &initially_missing_paths,
            write_or_edit_seen,
        ) {
            let mut import_scan_paths = changed_paths.clone();
            import_scan_paths.extend(required_paths.iter().cloned());
            let missing_imports =
                scan_relative_imports(&config.workspace_root, &import_scan_paths)?;
            if !missing_imports.is_empty() {
                last_blocking_reason = Some("missing relative imports".to_string());
                pending_feedback = Some(format_missing_import_feedback(&missing_imports));
                continue;
            }
            if let Some(contract) = completion_contract
                .as_ref()
                .filter(|_| contract_runtime_enabled)
                .filter(|contract| contract.has_verify())
            {
                if !batch_had_edit {
                    match handle_verify_repair_no_edit(
                        &config.workspace_root,
                        config.eval_events_path.as_deref(),
                        contract,
                        user_prompt,
                        &mut verify_repair_state,
                        &changed_paths,
                        &options,
                    )? {
                        VerifyRepairNoEditOutcome::NoPendingFailure => {}
                        VerifyRepairNoEditOutcome::Feedback(feedback) => {
                            last_blocking_reason = Some("verify repair missing edit".to_string());
                            pending_feedback = Some(feedback);
                            continue;
                        }
                        VerifyRepairNoEditOutcome::ObservationIncomplete(observation) => {
                            return Ok(RunSessionOutcome {
                                final_text: format!(
                                    "completion contract observed incomplete: {}",
                                    observation.primary_reason
                                ),
                                stop_reason: RunStopReason::CompletionContractObservedIncomplete,
                                changed_paths,
                                iterations: iteration + 1,
                                tool_calls: tool_call_count,
                                missing_required_paths: observation.missing_paths,
                                missing_capabilities: observation.missing_capabilities,
                                missing_evidence: observation.missing_evidence,
                                missing_obligations: observation.missing_obligations,
                                verify_attempts,
                                last_blocking_reason,
                                last_provider_error,
                            });
                        }
                    }
                }
                match verify_completion_contract_with_enforcement(
                    &config.workspace_root,
                    config.eval_events_path.as_deref(),
                    contract,
                    user_prompt,
                    &mut verify_attempts,
                    verify_repair_state.pending_signature.as_ref(),
                    verify_repair_state.pending_target,
                    &verify_repair_state.changed_paths_at_failure,
                    &changed_paths,
                    &batch_changed_paths,
                    batch_had_edit,
                    options.dependency_setup_authority,
                    config.offline,
                    &options,
                ) {
                    Ok(ContractVerificationOutcome::Satisfied) => {
                        eval_events::emit(
                            config.eval_events_path.as_deref(),
                            json!({
                                "event": "loop_stop",
                                "reason": "completion_contract_satisfied",
                                "required_paths": required_paths,
                                "verify_attempts": verify_attempts,
                            }),
                        );
                        return Ok(RunSessionOutcome {
                            final_text: format!(
                                "completion contract satisfied: {}",
                                required_paths.join(", ")
                            ),
                            stop_reason: RunStopReason::CompletionContractSatisfied,
                            changed_paths,
                            iterations: iteration + 1,
                            tool_calls: tool_call_count,
                            missing_required_paths: Vec::new(),
                            missing_capabilities: Vec::new(),
                            missing_evidence: Vec::new(),
                            missing_obligations: Vec::new(),
                            verify_attempts,
                            last_blocking_reason,
                            last_provider_error,
                        });
                    }
                    Ok(ContractVerificationOutcome::NeedsRepair(feedback)) => {
                        last_blocking_reason = Some("completion verify failed".to_string());
                        verify_repair_state.pending_error_context = feedback.error_context.clone();
                        verify_repair_state.pending_signature = Some(feedback.signature);
                        verify_repair_state.pending_target = Some(feedback.target);
                        verify_repair_state.changed_paths_at_failure = changed_paths.clone();
                        verify_repair_state.no_edit_turns = 0;
                        pending_feedback = Some(feedback.feedback);
                        continue;
                    }
                    Ok(ContractVerificationOutcome::ObservationIncomplete(observation)) => {
                        return Ok(RunSessionOutcome {
                            final_text: format!(
                                "completion contract observed incomplete: {}",
                                observation.primary_reason
                            ),
                            stop_reason: RunStopReason::CompletionContractObservedIncomplete,
                            changed_paths,
                            iterations: iteration + 1,
                            tool_calls: tool_call_count,
                            missing_required_paths: observation.missing_paths,
                            missing_capabilities: observation.missing_capabilities,
                            missing_evidence: observation.missing_evidence,
                            missing_obligations: observation.missing_obligations,
                            verify_attempts,
                            last_blocking_reason,
                            last_provider_error,
                        });
                    }
                    Err(err) => {
                        return Err(err);
                    }
                }
            }
            if options.contract_enforcement == ContractEnforcement::Enforce
                && let Some(gate) = step_capability_gate.as_ref()
                && let Some(feedback) = gate.maybe_feedback(
                    &config.workspace_root,
                    config.eval_events_path.as_deref(),
                    &required_paths,
                )
            {
                last_blocking_reason = Some("capability evidence missing".to_string());
                pending_feedback = Some(feedback);
                continue;
            }
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "loop_stop",
                    "reason": "required_artifacts_satisfied_after_tool",
                    "required_paths": required_paths,
                }),
            );
            return Ok(RunSessionOutcome {
                final_text: format!(
                    "required artifacts satisfied: {}",
                    required_paths.join(", ")
                ),
                stop_reason: RunStopReason::RequiredArtifactsSatisfiedAfterTool,
                changed_paths,
                iterations: iteration + 1,
                tool_calls: tool_call_count,
                missing_required_paths: Vec::new(),
                missing_capabilities: Vec::new(),
                missing_evidence: Vec::new(),
                missing_obligations: Vec::new(),
                verify_attempts,
                last_blocking_reason,
                last_provider_error,
            });
        }
        let mut missing = missing_after_batch;
        if !missing.is_empty()
            && remaining_iterations <= SETUP_SCAFFOLD_COMPLETION_REMAINING_THRESHOLD
        {
            maybe_complete_setup_scaffold(
                config,
                &options,
                user_prompt,
                &missing,
                SetupScaffoldCompletionTrigger::BudgetLow,
                &mut changed_paths,
            )?;
            missing = missing_paths(&config.workspace_root, &required_paths);
            if missing.is_empty() {
                return Ok(stop_for_required_artifacts_satisfied(
                    config,
                    &required_paths,
                    RequiredArtifactsStop {
                        changed_paths,
                        iterations: iterations_used,
                        tool_calls: tool_call_count,
                        verify_attempts,
                        last_blocking_reason,
                        last_provider_error,
                    },
                ));
            }
        }
        if maybe_rescue_artifact_recovery_exhaustion(
            config,
            &options,
            user_prompt,
            ArtifactRecoveryRescueInput {
                state: &artifact_recovery_state,
                non_edit_streak: artifact_non_edit_streak,
                enabled: artifact_recovery_enabled,
                missing_paths: &missing,
            },
            &mut changed_paths,
        )? {
            missing = missing_paths(&config.workspace_root, &required_paths);
            if missing.is_empty() {
                return Ok(stop_for_required_artifacts_satisfied(
                    config,
                    &required_paths,
                    RequiredArtifactsStop {
                        changed_paths,
                        iterations: iterations_used,
                        tool_calls: tool_call_count,
                        verify_attempts,
                        last_blocking_reason,
                        last_provider_error,
                    },
                ));
            }
        }
        if let Some(feedback) = maybe_artifact_recovery_feedback(
            &mut artifact_recovery_state,
            &mut artifact_non_edit_streak,
            ArtifactRecoveryFeedbackContext {
                eval_events_path: config.eval_events_path.as_deref(),
                enabled: artifact_recovery_enabled,
                missing_paths: &missing,
                required_paths: &required_paths,
                contract: completion_contract
                    .as_ref()
                    .filter(|_| contract_runtime_enabled),
                root: &config.workspace_root,
            },
        )? {
            last_blocking_reason = Some("artifact creation stalled".to_string());
            pending_feedback = Some(feedback);
            continue;
        }
    }
    let mut missing = missing_paths(&config.workspace_root, &required_paths);
    if !missing.is_empty() {
        maybe_complete_setup_scaffold(
            config,
            &options,
            user_prompt,
            &missing,
            SetupScaffoldCompletionTrigger::Exhausted,
            &mut changed_paths,
        )?;
        missing = missing_paths(&config.workspace_root, &required_paths);
        if missing.is_empty() {
            return Ok(stop_for_required_artifacts_satisfied(
                config,
                &required_paths,
                RequiredArtifactsStop {
                    changed_paths,
                    iterations: iteration_limit,
                    tool_calls: tool_call_count,
                    verify_attempts,
                    last_blocking_reason,
                    last_provider_error,
                },
            ));
        }
    }
    let non_scaffold_missing = non_scaffold_missing_paths(config, &missing);
    let artifact_stagnation_feedback_count = artifact_recovery_state.target_attempts;
    let mut exhaustion_context = verify_repair_state.pending_error_context.clone();
    let pending_contract_keys = pending_contract_keys_from_error_context(&exhaustion_context);
    let capability_exhaustion_reason = if missing.is_empty() {
        super::feedback::capability_evidence_unresolved_reason(&pending_contract_keys)
    } else {
        None
    };
    let read_only_stagnation_reason =
        if missing.is_empty() && read_only_streak >= READ_ONLY_STAGNATION_INTERVENTION_THRESHOLD {
            Some("model_stagnation:read_only_loop".to_string())
        } else {
            None
        };
    let no_progress_stagnation_reason = if missing.is_empty()
        && read_only_stagnation_reason.is_none()
        && capability_exhaustion_reason.is_none()
        && last_blocking_reason.is_none()
        && last_provider_error.is_none()
    {
        Some("model_stagnation:no_progress_recorded".to_string())
    } else {
        None
    };
    let reason = if !non_scaffold_missing.is_empty() {
        "artifact_follow_through_exhausted".to_string()
    } else if let Some(reason) = &capability_exhaustion_reason {
        reason.clone()
    } else if let Some(reason) = &read_only_stagnation_reason {
        reason.clone()
    } else if let Some(reason) = &no_progress_stagnation_reason {
        reason.clone()
    } else if missing.is_empty() {
        "loop_progress_exhausted".to_string()
    } else {
        "scaffold_artifact_follow_through_exhausted".to_string()
    };
    let loop_stop_event = json!({
            "event": "loop_stop",
            "reason": reason.clone(),
            "missing_paths": missing,
            "non_scaffold_missing_paths": non_scaffold_missing.clone(),
            "missing_capabilities": exhaustion_context.missing_capabilities.clone(),
            "missing_evidence": exhaustion_context.missing_evidence.clone(),
            "missing_obligations": exhaustion_context.missing_obligations.clone(),
            "artifact_stagnation_feedback_count": artifact_stagnation_feedback_count,
            "read_only_streak": read_only_streak,
            "verify_attempts": verify_attempts,
            "last_blocking_reason": last_blocking_reason,
            "last_provider_error": last_provider_error.as_deref().map(eval_events::body_snippet),
    });
    eval_events::emit(config.eval_events_path.as_deref(), loop_stop_event);
    if !non_scaffold_missing.is_empty() {
        bail!(
            "artifact_follow_through_exhausted: missing expected paths: {}; artifact_stagnation_feedback_count: {}",
            non_scaffold_missing.join(", "),
            artifact_stagnation_feedback_count
        );
    }
    if let Some(reason) = capability_exhaustion_reason {
        if exhaustion_context.repair_target.is_none() {
            exhaustion_context.repair_target = Some("required_evidence_missing".to_string());
        }
        return Err(RunSessionError::new(reason, exhaustion_context).into());
    }
    if let Some(reason) = read_only_stagnation_reason {
        let objective = read_only_objective_excerpt(user_prompt);
        bail!("{reason}: objective: {objective}");
    }
    if let Some(reason) = no_progress_stagnation_reason {
        let objective = read_only_objective_excerpt(user_prompt);
        bail!("{reason}: objective: {objective}");
    }
    let blocker = if missing.is_empty() {
        last_blocking_reason
            .as_deref()
            .unwrap_or("progress stalled without a recorded artifact, capability, environment, or tool blocker")
            .to_string()
    } else {
        format!("missing scaffold paths: {}", missing.join(", "))
    };
    if exhaustion_context.is_empty() {
        bail!("{reason}: {blocker}")
    }
    Err(RunSessionError::new(format!("{reason}: {blocker}"), exhaustion_context).into())
}

fn pending_contract_keys_from_error_context(context: &RunSessionErrorContext) -> Vec<String> {
    let mut keys = Vec::new();
    for key in &context.missing_capabilities {
        push_unique(&mut keys, key.clone());
    }
    for key in &context.missing_evidence {
        push_unique(&mut keys, key.clone());
    }
    for key in &context.missing_obligations {
        push_unique(&mut keys, key.clone());
    }
    keys
}

fn missing_paths(root: &std::path::Path, required_paths: &[String]) -> Vec<String> {
    required_paths
        .iter()
        .filter(|path| resolve_existing(root, path).is_err())
        .cloned()
        .collect()
}

fn maybe_short_circuit_satisfied_step(
    config: &Config,
    options: &RunSessionOptions,
    user_prompt: &str,
    required_paths: &[String],
    contract: Option<&CompletionContract>,
    context: ShortCircuitContext<'_>,
) -> anyhow::Result<Option<RunSessionOutcome>> {
    let ShortCircuitContext {
        verify_attempts,
        at,
        write_or_edit_seen,
    } = context;
    if at == StepShortCircuitAt::Start && options.step_kind == Some(RunSessionStepKind::Implement) {
        return Ok(None);
    }
    let has_contract_gate = contract.is_some_and(CompletionContract::has_verify);
    if required_paths.is_empty() {
        return Ok(None);
    }
    let missing = missing_paths(&config.workspace_root, required_paths);
    if !missing.is_empty() {
        return Ok(None);
    }
    if let Some(contract) = contract.filter(|_| has_contract_gate) {
        if options.require_mutation_before_contract_short_circuit && !write_or_edit_seen {
            return Ok(None);
        }
        let attempts_before_probe = *verify_attempts;
        match verify_completion_contract_with_enforcement(
            &config.workspace_root,
            config.eval_events_path.as_deref(),
            contract,
            user_prompt,
            verify_attempts,
            None,
            None,
            &[],
            &[],
            &[],
            false,
            options.dependency_setup_authority,
            config.offline,
            options,
        )? {
            ContractVerificationOutcome::Satisfied => {
                emit_step_short_circuited(config, options, required_paths, *verify_attempts, at);
                return Ok(Some(RunSessionOutcome {
                    final_text: format!("step short-circuited: {}", required_paths.join(", ")),
                    stop_reason: RunStopReason::CompletionContractSatisfied,
                    changed_paths: Vec::new(),
                    iterations: 0,
                    tool_calls: 0,
                    missing_required_paths: Vec::new(),
                    missing_capabilities: Vec::new(),
                    missing_evidence: Vec::new(),
                    missing_obligations: Vec::new(),
                    verify_attempts: *verify_attempts,
                    last_blocking_reason: None,
                    last_provider_error: None,
                }));
            }
            ContractVerificationOutcome::NeedsRepair(_)
            | ContractVerificationOutcome::ObservationIncomplete(_) => {
                *verify_attempts = attempts_before_probe;
                return Ok(None);
            }
        }
    }
    if !setup_short_circuit_allowed(options, user_prompt) {
        return Ok(None);
    }
    emit_step_short_circuited(config, options, required_paths, *verify_attempts, at);
    Ok(Some(RunSessionOutcome {
        final_text: format!("step short-circuited: {}", required_paths.join(", ")),
        stop_reason: RunStopReason::RequiredArtifactsSatisfiedAfterTool,
        changed_paths: Vec::new(),
        iterations: 0,
        tool_calls: 0,
        missing_required_paths: Vec::new(),
        missing_capabilities: Vec::new(),
        missing_evidence: Vec::new(),
        missing_obligations: Vec::new(),
        verify_attempts: *verify_attempts,
        last_blocking_reason: None,
        last_provider_error: None,
    }))
}

fn emit_step_short_circuited(
    config: &Config,
    options: &RunSessionOptions,
    required_paths: &[String],
    verify_attempts: usize,
    at: StepShortCircuitAt,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "step_short_circuited",
            "at": at.as_str(),
            "required_paths": required_paths,
            "verify_attempts": verify_attempts,
            "session_scope": options.scope.as_str(),
            "step_kind": options.step_kind.map(RunSessionStepKind::as_str).unwrap_or(""),
            "phase_scope": options.phase_scope.as_deref().unwrap_or(""),
        }),
    );
}

fn setup_short_circuit_allowed(options: &RunSessionOptions, user_prompt: &str) -> bool {
    if options.step_kind != Some(RunSessionStepKind::Setup) {
        return false;
    }
    let instruction = current_step_instruction(user_prompt).unwrap_or(user_prompt);
    if setup_short_circuit_instruction(instruction) {
        return true;
    }
    let phase_scope = options
        .phase_scope
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    phase_scope.contains("setup") || phase_scope.contains("scaffold")
}

fn current_step_instruction(prompt: &str) -> Option<&str> {
    let marker = "Current step instruction:\n";
    let after_marker = prompt.split_once(marker)?.1;
    Some(
        after_marker
            .split("\n\n")
            .next()
            .unwrap_or(after_marker)
            .trim(),
    )
}

fn setup_short_circuit_instruction(instruction: &str) -> bool {
    let lower = instruction.to_ascii_lowercase();
    lower.contains("setup")
        || lower.contains("set up")
        || lower.contains("scaffold")
        || lower.contains("pre-scaffold")
        || lower.contains("pre-provision")
        || lower.contains("already present")
}

fn non_scaffold_missing_paths(config: &Config, missing_paths: &[String]) -> Vec<String> {
    let scaffold_paths = profile_setup_scaffold_paths(&config.workspace_root, &config.profile)
        .into_iter()
        .collect::<BTreeSet<_>>();
    missing_paths
        .iter()
        .filter(|path| {
            !scaffold_paths.contains(*path) && !python_cli_entrypoint_scaffold_path(config, path)
        })
        .cloned()
        .collect()
}

fn maybe_complete_setup_scaffold(
    config: &Config,
    options: &RunSessionOptions,
    user_prompt: &str,
    missing_paths: &[String],
    trigger: SetupScaffoldCompletionTrigger,
    changed_paths: &mut Vec<String>,
) -> anyhow::Result<Vec<String>> {
    if !setup_scaffold_completion_applicable(config, options, user_prompt, missing_paths) {
        return Ok(Vec::new());
    }
    let created =
        profile_complete_scaffold(&config.workspace_root, &config.profile, missing_paths)?;
    if created.is_empty() {
        return Ok(created);
    }
    for path in &created {
        if !changed_paths.contains(path) {
            changed_paths.push(path.clone());
        }
    }
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "setup_scaffold_completed",
            "paths": created,
            "trigger": trigger.as_str(),
            "profile": config.profile,
            "session_scope": options.scope.as_str(),
            "step_kind": options.step_kind.map(RunSessionStepKind::as_str).unwrap_or(""),
            "phase_scope": options.phase_scope.as_deref().unwrap_or(""),
        }),
    );
    Ok(created)
}

fn setup_scaffold_completion_applicable(
    config: &Config,
    options: &RunSessionOptions,
    user_prompt: &str,
    missing_paths: &[String],
) -> bool {
    if missing_paths.is_empty() || !setup_step_or_phase(options, user_prompt) {
        return false;
    }
    let scaffold_paths = profile_setup_scaffold_paths(&config.workspace_root, &config.profile)
        .into_iter()
        .collect::<BTreeSet<_>>();
    if scaffold_paths.is_empty() {
        return false;
    }
    missing_paths.iter().all(|path| {
        scaffold_paths.contains(path) || python_cli_entrypoint_scaffold_path(config, path)
    })
}

struct ArtifactRecoveryRescueInput<'a> {
    state: &'a ArtifactRecoveryState,
    non_edit_streak: usize,
    enabled: bool,
    missing_paths: &'a [String],
}

fn maybe_rescue_artifact_recovery_exhaustion(
    config: &Config,
    options: &RunSessionOptions,
    user_prompt: &str,
    input: ArtifactRecoveryRescueInput<'_>,
    changed_paths: &mut Vec<String>,
) -> anyhow::Result<bool> {
    if !input.enabled
        || input.missing_paths.is_empty()
        || input.non_edit_streak < ARTIFACT_NON_EDIT_STAGNATION_THRESHOLD
        || input.state.target_attempts < ARTIFACT_RECOVERY_ATTEMPT_LIMIT
    {
        return Ok(false);
    }
    let created = maybe_complete_setup_scaffold(
        config,
        options,
        user_prompt,
        input.missing_paths,
        SetupScaffoldCompletionTrigger::Exhausted,
        changed_paths,
    )?;
    Ok(!created.is_empty())
}

fn setup_step_or_phase(options: &RunSessionOptions, user_prompt: &str) -> bool {
    if options.step_kind == Some(RunSessionStepKind::Setup) {
        return true;
    }
    if options.step_kind != Some(RunSessionStepKind::Implement) {
        return false;
    }
    let lower = format!(
        "{}\n{}",
        options.phase_scope.as_deref().unwrap_or(""),
        user_prompt
    )
    .to_ascii_lowercase();
    [
        "setup",
        "set up",
        "scaffold",
        "initialize",
        "initialise",
        "init",
    ]
    .iter()
    .any(|token| lower.contains(token))
}

fn python_cli_entrypoint_scaffold_path(config: &Config, path: &str) -> bool {
    if crate::planner::profile::canonical_profile_name(&config.profile) != "python-cli" {
        return false;
    }
    let normalized = path.replace('\\', "/");
    normalized.starts_with("src/")
        && normalized.ends_with("/main.py")
        && normalized
            .strip_prefix("src/")
            .and_then(|tail| tail.strip_suffix("/main.py"))
            .is_some_and(|package| {
                !package.is_empty()
                    && !package.chars().next().is_some_and(|ch| ch.is_ascii_digit())
                    && package
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            })
}

struct RequiredArtifactsStop {
    changed_paths: Vec<String>,
    iterations: usize,
    tool_calls: usize,
    verify_attempts: usize,
    last_blocking_reason: Option<String>,
    last_provider_error: Option<String>,
}

fn stop_for_required_artifacts_satisfied(
    config: &Config,
    required_paths: &[String],
    stop: RequiredArtifactsStop,
) -> RunSessionOutcome {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "loop_stop",
            "reason": "required_artifacts_satisfied_after_tool",
            "required_paths": required_paths,
        }),
    );
    RunSessionOutcome {
        final_text: format!(
            "required artifacts satisfied: {}",
            required_paths.join(", ")
        ),
        stop_reason: RunStopReason::RequiredArtifactsSatisfiedAfterTool,
        changed_paths: stop.changed_paths,
        iterations: stop.iterations,
        tool_calls: stop.tool_calls,
        missing_required_paths: Vec::new(),
        missing_capabilities: Vec::new(),
        missing_evidence: Vec::new(),
        missing_obligations: Vec::new(),
        verify_attempts: stop.verify_attempts,
        last_blocking_reason: stop.last_blocking_reason,
        last_provider_error: stop.last_provider_error,
    }
}

#[derive(Debug, Clone)]
struct StepCapabilityGate {
    required_capabilities: Vec<String>,
    required_evidence: Vec<String>,
}

impl StepCapabilityGate {
    fn from_prompt(prompt: &str, options: &RunSessionOptions) -> Option<Self> {
        if options.scope != RunSessionScope::PlanRunStep
            || options.step_kind != Some(RunSessionStepKind::Implement)
        {
            return None;
        }
        let required_capabilities =
            parse_prompt_bullet_section(prompt, "Required final capabilities:");
        let required_evidence = parse_prompt_bullet_section(prompt, "Required final evidence:")
            .into_iter()
            .filter(|evidence| capability_step_evidence(evidence))
            .collect::<Vec<_>>();
        if !required_capabilities
            .iter()
            .any(|capability| interactive_capability(capability))
            && !required_evidence
                .iter()
                .any(|evidence| interactive_evidence(evidence))
        {
            return None;
        }
        Some(Self {
            required_capabilities,
            required_evidence,
        })
    }

    fn maybe_feedback(
        &self,
        root: &Path,
        eval_events_path: Option<&Path>,
        required_paths: &[String],
    ) -> Option<String> {
        let report = verify_runtime_acceptance(
            root,
            required_paths,
            &[],
            &self.required_capabilities,
            &self.required_evidence,
            &["implementation".to_string()],
            &[],
        );
        eval_events::emit(
            eval_events_path,
            json!({
                "event": "step_capability_evidence_check",
                "ok": report.passed,
                "required_capabilities": self.required_capabilities.clone(),
                "required_evidence": self.required_evidence.clone(),
                "missing_capabilities": report.missing_capabilities.clone(),
                "missing_evidence": report.missing_evidence.clone(),
                "missing_obligations": report.missing_obligations.clone(),
                "weak_evidence": report.weak_evidence.clone(),
                "browser_readiness_status": report.browser_readiness_status.clone(),
                "browser_readiness_evidence_path": report.browser_readiness_evidence_path.clone(),
                "interaction_evidence_status": report.interaction_evidence_status.clone(),
                "interaction_evidence_path": report.interaction_evidence_path.clone(),
                "primary_reason": eval_events::body_snippet(&report.primary_reason),
            }),
        );
        if report.passed {
            return None;
        }
        Some(super::feedback::missing_capability_evidence(
            &report.missing_evidence,
            &report.missing_capabilities,
        ))
    }
}

fn parse_prompt_bullet_section(prompt: &str, header: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_section = false;
    for line in prompt.lines() {
        let trimmed = line.trim();
        if trimmed == header {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        if trimmed.is_empty() {
            if out.is_empty() {
                continue;
            }
            break;
        }
        let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        else {
            break;
        };
        let item = item.trim();
        if !item.is_empty() && item != "none" {
            out.push(item.to_string());
        }
    }
    out
}

fn interactive_capability(capability: &str) -> bool {
    matches!(
        capability.trim(),
        "browser_interaction"
            | "playable_ui"
            | "stateful_interaction"
            | "start_or_restart_flow"
            | "player_control"
            | "adversary_or_challenge"
            | "progression_or_score"
            | "failure_or_collision_rule"
            | "user_input_or_action"
            | "visible_state_change"
    )
}

fn capability_step_evidence(evidence: &str) -> bool {
    !matches!(
        evidence.trim(),
        "nextjs_route_evidence" | "build_command_or_dependency_missing_boundary"
    )
}

fn interactive_evidence(evidence: &str) -> bool {
    matches!(
        evidence.trim(),
        "interactive_ui_source_evidence"
            | "non_static_screen_evidence"
            | "visible_interactive_surface_evidence"
            | "user_input_handler_evidence"
            | "stateful_update_evidence"
            | "challenge_or_adversary_evidence"
            | "score_or_progression_evidence"
            | "failure_or_collision_evidence"
            | "restart_or_recoverable_state_evidence"
    )
}

#[derive(Debug, Clone)]
struct RequiredPathSources {
    explicit_required_paths: Vec<String>,
    prompt_extracted_paths: Vec<String>,
    completion_contract_paths: Vec<String>,
    effective_required_paths: Vec<String>,
}

fn effective_required_path_sources(
    root: &Path,
    explicit: &[String],
    prompt: &str,
    contract_paths: &[String],
    options: &RunSessionOptions,
) -> RequiredPathSources {
    let prompt_extracted_paths = if options.prompt_artifact_extraction_enabled() {
        extract_requested_artifact_paths(root, prompt)
    } else {
        Vec::new()
    };
    let completion_contract_paths = if options.contract_path_merge_enabled() {
        contract_paths.to_vec()
    } else {
        Vec::new()
    };
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for path in explicit.iter().cloned() {
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    for path in prompt_extracted_paths.iter().cloned() {
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    for path in completion_contract_paths.iter().cloned() {
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    RequiredPathSources {
        explicit_required_paths: explicit.to_vec(),
        prompt_extracted_paths,
        completion_contract_paths,
        effective_required_paths: out,
    }
}

fn tool_context_expected_paths(
    required_paths: &[String],
    options: &RunSessionOptions,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for path in required_paths
        .iter()
        .chain(options.path_fallback_candidates.iter())
    {
        if seen.insert(path.clone()) {
            out.push(path.clone());
        }
    }
    out
}

fn emit_step_obligation_scope(
    eval_events_path: Option<&Path>,
    options: &RunSessionOptions,
    sources: &RequiredPathSources,
    initially_missing_paths: &[String],
) {
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "step_obligation_scope",
            "session_scope": options.scope.as_str(),
            "step_kind": options.step_kind.map(RunSessionStepKind::as_str).unwrap_or(""),
            "explicit_required_paths": sources.explicit_required_paths.clone(),
            "prompt_extracted_paths_enabled": options.prompt_artifact_extraction_enabled(),
            "prompt_extracted_paths": sources.prompt_extracted_paths.clone(),
            "completion_contract_path_merge_enabled": options.contract_path_merge_enabled(),
            "completion_contract_verification_enabled": options.contract_runtime_enabled(),
            "contract_enforcement": options.contract_enforcement_label(),
            "phase_scope": options.phase_scope.as_deref().unwrap_or(""),
            "completion_contract_paths": sources.completion_contract_paths.clone(),
            "effective_required_paths": sources.effective_required_paths.clone(),
            "initially_missing_paths": initially_missing_paths,
            "contract_paths_merged": options.contract_path_merge_enabled()
                && !sources.completion_contract_paths.is_empty(),
        }),
    );
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn verify_completion_contract(
    root: &Path,
    eval_events_path: Option<&Path>,
    contract: &CompletionContract,
    goal: &str,
    verify_attempts: &mut usize,
    previous_signature: Option<&VerificationSignature>,
    previous_target: Option<RepairTarget>,
    changed_paths_before: &[String],
    changed_paths_after: &[String],
    repair_turn_changed_paths: &[String],
    had_edit: bool,
    setup_authority: NodeDependencySetupAuthority,
    offline: bool,
) -> anyhow::Result<Option<VerifyFailureFeedback>> {
    let options = RunSessionOptions::default();
    match verify_completion_contract_with_enforcement(
        root,
        eval_events_path,
        contract,
        goal,
        verify_attempts,
        previous_signature,
        previous_target,
        changed_paths_before,
        changed_paths_after,
        repair_turn_changed_paths,
        had_edit,
        setup_authority,
        offline,
        &options,
    )? {
        ContractVerificationOutcome::Satisfied => Ok(None),
        ContractVerificationOutcome::NeedsRepair(feedback) => Ok(Some(feedback)),
        ContractVerificationOutcome::ObservationIncomplete(_) => {
            unreachable!("default completion contract enforcement cannot observe")
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn verify_completion_contract_with_enforcement(
    root: &Path,
    eval_events_path: Option<&Path>,
    contract: &CompletionContract,
    goal: &str,
    verify_attempts: &mut usize,
    previous_signature: Option<&VerificationSignature>,
    previous_target: Option<RepairTarget>,
    changed_paths_before: &[String],
    changed_paths_after: &[String],
    repair_turn_changed_paths: &[String],
    had_edit: bool,
    setup_authority: NodeDependencySetupAuthority,
    offline: bool,
    options: &RunSessionOptions,
) -> anyhow::Result<ContractVerificationOutcome> {
    *verify_attempts += 1;
    let (report, build_verifier_lifecycles) = contract
        .verify_with_goal_observed_with_setup_authority(root, goal, setup_authority, offline);
    let build_verifier_observations = build_verifier_lifecycles
        .iter()
        .map(|lifecycle| lifecycle.final_observation().clone())
        .collect::<Vec<_>>();
    let runtime_acceptance = contract.runtime_acceptance_report(root);
    let ok = report.is_pass();
    let (signature, verdict) = classify_repair_progress(previous_signature, &report, had_edit);
    let repair_target = classify_repair_target(&report);
    let repair_follow_through = previous_target
        .map(|target| classify_repair_follow_through(target, repair_turn_changed_paths));
    let repair_follow_through_label = repair_follow_through
        .map(RepairFollowThrough::as_str)
        .unwrap_or("");
    let repair_failure_kind = repair_follow_through
        .and_then(RepairFollowThrough::failure_kind)
        .unwrap_or("");
    let repair_target_followed = repair_follow_through.map(RepairFollowThrough::followed);
    let previous_repair_target = previous_target.map(RepairTarget::as_str).unwrap_or("");
    let build_verifier_required = build_verifier_lifecycles
        .iter()
        .any(|lifecycle| lifecycle.requirement.required_for_completion);
    let build_verifier_attempted = build_verifier_lifecycles.iter().any(|lifecycle| {
        lifecycle.before_setup.attempted
            || lifecycle
                .after_setup
                .as_ref()
                .is_some_and(|observation| observation.attempted)
            || lifecycle
                .setup
                .as_ref()
                .is_some_and(|setup| setup.attempted)
    });
    let build_verifier_statuses = build_verifier_observations
        .iter()
        .map(|observation| format!("{}:{}", observation.command, observation.status_str()))
        .collect::<Vec<_>>();
    let dependency_setup_status = dependency_setup_status(&build_verifier_lifecycles);
    let verifier_bootstrap_state = verifier_bootstrap::state_from_lifecycles(
        build_verifier_required,
        &build_verifier_lifecycles,
    );
    let reachability =
        assess_repair_reachability(&report, Some(contract), setup_authority, offline);
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "completion_verify",
            "ok": ok,
            "attempt": *verify_attempts,
            "repair_cap": contract.verify_repair_cap,
            "contract_enforcement": options.contract_enforcement_label(),
            "session_scope": options.scope.as_str(),
            "step_kind": options.step_kind.map(RunSessionStepKind::as_str).unwrap_or(""),
            "phase_scope": options.phase_scope.as_deref().unwrap_or(""),
            "missing_paths": report.missing_paths.clone(),
            "command_failures": report.command_failures.len(),
            "verifier_command_false_negatives": report.verifier_command_false_negatives.clone(),
            "dependency_missing": report.dependency_missing.clone(),
            "compile_errors": report.compile_errors.clone(),
            "profile": contract.profile.as_deref().unwrap_or(""),
            "profile_failures": report.profile_failures.clone(),
            "required_capabilities": contract.required_capabilities.clone(),
            "required_evidence": contract.required_evidence.clone(),
            "required_obligations": contract.required_obligations.clone(),
            "missing_capabilities": runtime_acceptance.missing_capabilities.clone(),
            "missing_evidence": runtime_acceptance.missing_evidence.clone(),
            "missing_obligations": runtime_acceptance.missing_obligations.clone(),
            "weak_evidence": runtime_acceptance.weak_evidence.clone(),
            "evidence_tiers": runtime_acceptance.evidence_tiers.clone(),
            "artifact_obligations": runtime_acceptance.artifact_obligations.clone(),
            "capability_evidence_bindings": runtime_acceptance.capability_evidence_bindings.clone(),
            "obligation_repair_targets": runtime_acceptance.obligation_repair_targets.clone(),
            "inconclusive_reasons": runtime_acceptance.inconclusive_reasons.clone(),
            "browser_readiness_status": runtime_acceptance.browser_readiness_status.clone(),
            "browser_readiness_evidence_path": runtime_acceptance.browser_readiness_evidence_path.clone(),
            "interaction_evidence_status": runtime_acceptance.interaction_evidence_status.clone(),
            "interaction_evidence_path": runtime_acceptance.interaction_evidence_path.clone(),
            "runtime_acceptance_passed": runtime_acceptance.passed,
            "runtime_acceptance_inconclusive": runtime_acceptance.inconclusive,
            "runtime_acceptance_primary_reason": eval_events::body_snippet(&runtime_acceptance.primary_reason),
            "deferred_verify_requirements": contract.deferred_status_summary(root, goal),
            "build_verifier_required": build_verifier_required,
            "build_verifier_attempted": build_verifier_attempted,
            "build_verifier_statuses": build_verifier_statuses,
            "build_verifier_observations": build_verifier_observations.clone(),
            "build_verifier_lifecycle": build_verifier_lifecycles.clone(),
            "dependency_setup_status": dependency_setup_status,
            "dependency_setup_authority": setup_authority.as_str(),
            "verifier_bootstrap_state": verifier_bootstrap_state.as_str(),
            "repair_reachable": reachability.reachable,
            "reachable": reachability.reachable,
            "viable_actions": reachability_action_labels(&reachability),
            "blocked_requirements": reachability.blocked_requirements.clone(),
            "repair_target": repair_target.as_str(),
            "primary_reason": eval_events::body_snippet(&report.primary_reason()),
            "failure_signature": signature.label(),
            "repair_progress": verdict.as_str(),
        }),
    );
    if previous_target.is_some() {
        eval_events::emit(
            eval_events_path,
            json!({
                "event": "step_verify_repair",
                "mode": "minimal-loop",
                "attempt": *verify_attempts,
                "ok": ok,
                "repair_target": repair_target.as_str(),
                "previous_repair_target": previous_repair_target,
                "repair_target_followed": repair_target_followed,
                "target_relation": repair_follow_through_label,
                "repair_follow_through": repair_follow_through_label,
                "failure_kind": repair_failure_kind,
                "changed_paths_before": changed_paths_before,
                "changed_paths_after": changed_paths_after,
                "repair_turn_changed_paths": repair_turn_changed_paths,
                "allowed_action": previous_target.map(RepairTarget::allowed_action).unwrap_or(""),
                "primary_reason": eval_events::body_snippet(&report.primary_reason()),
            }),
        );
    }
    for lifecycle in &build_verifier_lifecycles {
        super::build_verifier::emit_dependency_build_lifecycle(
            eval_events_path,
            "minimal-loop",
            None,
            lifecycle,
        );
    }
    if ok {
        return Ok(ContractVerificationOutcome::Satisfied);
    }
    if !reachability.reachable {
        let failure_kind = reachability_failure_kind(&reachability).to_string();
        let recovery_reason = reachability_recovery_reason(&reachability);
        if options.contract_enforcement == ContractEnforcement::Observe {
            let observation = ContractObservation::from_report(&report, &runtime_acceptance);
            emit_contract_observation_incomplete(
                eval_events_path,
                options,
                contract,
                &observation,
                *verify_attempts,
                "repair_unreachable",
                &reachability.blocked_requirements,
            );
            return Ok(ContractVerificationOutcome::ObservationIncomplete(
                observation,
            ));
        }
        let recovery_paths = save_minimal_recovery_handoff(
            root,
            eval_events_path,
            contract,
            goal,
            &failure_kind,
            &recovery_reason,
            &report,
            changed_paths_after,
            repair_target,
        );
        eval_events::emit(
            eval_events_path,
            json!({
                "event": "repair_unreachable",
                "mode": "minimal-loop",
                "reason": failure_kind,
                "blocked_requirements": reachability.blocked_requirements.clone(),
                "viable_actions": reachability_action_labels(&reachability),
                "repair_target": repair_target.as_str(),
                "primary_reason": eval_events::body_snippet(&recovery_reason),
                "recovery_prompt_path": recovery_paths
                    .as_ref()
                    .map(|paths| paths.prompt_path.clone())
                    .unwrap_or_default(),
                "recovery_ultra_plan_path": recovery_paths
                    .as_ref()
                    .map(|paths| paths.yaml_path.clone())
                    .unwrap_or_default(),
                "recovery_yaml_missing": recovery_paths.is_none(),
            }),
        );
        eval_events::emit(
            eval_events_path,
            json!({
                "event": "loop_stop",
                "reason": failure_kind,
                "verify_attempts": *verify_attempts,
                "primary_reason": eval_events::body_snippet(&recovery_reason),
                "repair_target": repair_target.as_str(),
                "repair_reachable": false,
                "blocked_requirements": reachability.blocked_requirements.clone(),
                "recovery_prompt_path": recovery_paths
                    .as_ref()
                    .map(|paths| paths.prompt_path.clone())
                    .unwrap_or_default(),
                "recovery_ultra_plan_path": recovery_paths
                    .as_ref()
                    .map(|paths| paths.yaml_path.clone())
                    .unwrap_or_default(),
                "recovery_yaml_missing": recovery_paths.is_none(),
            }),
        );
        return Err(RunSessionError::new(
            render_minimal_recovery_stop_reason(
                format!("completion contract verify repair unreachable: {recovery_reason}"),
                recovery_paths.as_ref(),
            ),
            RunSessionErrorContext::from_runtime_acceptance(&runtime_acceptance, repair_target),
        )
        .into());
    }
    if previous_signature.is_some() {
        eval_events::emit(
            eval_events_path,
            json!({
                "event": "verify_repair_progress",
                "verdict": verdict.as_str(),
                "previous_signature": previous_signature.map(VerificationSignature::label).unwrap_or_default(),
                "current_signature": signature.label(),
                "had_edit": had_edit,
            }),
        );
    }
    let effective_repair_cap =
        if previous_signature.is_some() && matches!(verdict, RepairProgressVerdict::Improved) {
            contract.verify_repair_cap.saturating_add(1)
        } else {
            contract.verify_repair_cap
        };
    if *verify_attempts >= effective_repair_cap {
        let stop_reason = terminal_verify_stop_reason(
            &report,
            &signature,
            previous_signature,
            verdict,
            repair_target,
        );
        if options.contract_enforcement == ContractEnforcement::Observe {
            let observation = ContractObservation::from_report(&report, &runtime_acceptance);
            emit_contract_observation_incomplete(
                eval_events_path,
                options,
                contract,
                &observation,
                *verify_attempts,
                &stop_reason,
                &reachability.blocked_requirements,
            );
            return Ok(ContractVerificationOutcome::ObservationIncomplete(
                observation,
            ));
        }
        let recovery_paths = save_minimal_recovery_handoff(
            root,
            eval_events_path,
            contract,
            goal,
            &stop_reason,
            &report.primary_reason(),
            &report,
            changed_paths_after,
            repair_target,
        );
        eval_events::emit(
            eval_events_path,
            json!({
                "event": "loop_stop",
                "reason": stop_reason,
                "verify_attempts": *verify_attempts,
                "primary_reason": eval_events::body_snippet(&report.primary_reason()),
                "missing_capabilities": runtime_acceptance.missing_capabilities.clone(),
                "missing_evidence": runtime_acceptance.missing_evidence.clone(),
                "missing_obligations": runtime_acceptance.missing_obligations.clone(),
                "weak_evidence": runtime_acceptance.weak_evidence.clone(),
                "inconclusive_reasons": runtime_acceptance.inconclusive_reasons.clone(),
                "browser_readiness_status": runtime_acceptance.browser_readiness_status.clone(),
                "browser_readiness_evidence_path": runtime_acceptance.browser_readiness_evidence_path.clone(),
                "interaction_evidence_status": runtime_acceptance.interaction_evidence_status.clone(),
                "interaction_evidence_path": runtime_acceptance.interaction_evidence_path.clone(),
                "runtime_acceptance_inconclusive": runtime_acceptance.inconclusive,
                "repair_progress": verdict.as_str(),
                "failure_signature": signature.label(),
                "repair_target": repair_target.as_str(),
                "previous_repair_target": previous_repair_target,
                "repair_target_followed": repair_target_followed,
                "target_relation": repair_follow_through_label,
                "repair_follow_through": repair_follow_through_label,
                "changed_paths_before": changed_paths_before,
                "changed_paths_after": changed_paths_after,
                "repair_turn_changed_paths": repair_turn_changed_paths,
                "dependency_setup_status": dependency_setup_status,
                "verifier_bootstrap_state": verifier_bootstrap_state.as_str(),
                "recovery_prompt_path": recovery_paths
                    .as_ref()
                    .map(|paths| paths.prompt_path.clone())
                    .unwrap_or_default(),
                "recovery_ultra_plan_path": recovery_paths
                    .as_ref()
                    .map(|paths| paths.yaml_path.clone())
                    .unwrap_or_default(),
                "recovery_yaml_missing": recovery_paths.is_none(),
                "build_verifier_lifecycle": build_verifier_lifecycles.clone(),
                "build_verifier_statuses": build_verifier_observations
                    .iter()
                    .map(|observation| format!("{}:{}", observation.command, observation.status_str()))
                    .collect::<Vec<_>>(),
            }),
        );
        return Err(RunSessionError::new(
            render_minimal_recovery_stop_reason(
                format!(
                    "completion contract verify failed after {} attempts: {}",
                    *verify_attempts,
                    report.primary_reason()
                ),
                recovery_paths.as_ref(),
            ),
            RunSessionErrorContext::from_runtime_acceptance(&runtime_acceptance, repair_target),
        )
        .into());
    }
    let feedback = reanchored_verify_feedback_if_needed(
        format_verify_feedback_with_contract(&report, Some(contract)),
        repair_follow_through,
        repair_target,
        &report,
        contract,
        &runtime_acceptance,
    );
    Ok(ContractVerificationOutcome::NeedsRepair(
        VerifyFailureFeedback {
            feedback,
            signature,
            target: repair_target,
            error_context: RunSessionErrorContext::from_runtime_acceptance(
                &runtime_acceptance,
                repair_target,
            ),
        },
    ))
}

fn emit_contract_observation_incomplete(
    eval_events_path: Option<&Path>,
    options: &RunSessionOptions,
    contract: &CompletionContract,
    observation: &ContractObservation,
    verify_attempts: usize,
    reason: &str,
    blocked_requirements: &[String],
) {
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "contract_observation_incomplete",
            "contract_enforcement": options.contract_enforcement_label(),
            "session_scope": options.scope.as_str(),
            "step_kind": options.step_kind.map(RunSessionStepKind::as_str).unwrap_or(""),
            "phase_scope": options.phase_scope.as_deref().unwrap_or(""),
            "verify_attempts": verify_attempts,
            "repair_cap": contract.verify_repair_cap,
            "reason": reason,
            "required_paths": contract.required_paths.clone(),
            "missing_paths": observation.missing_paths.clone(),
            "required_capabilities": contract.required_capabilities.clone(),
            "required_evidence": contract.required_evidence.clone(),
            "required_obligations": contract.required_obligations.clone(),
            "missing_capabilities": observation.missing_capabilities.clone(),
            "missing_evidence": observation.missing_evidence.clone(),
            "missing_obligations": observation.missing_obligations.clone(),
            "primary_reason": eval_events::body_snippet(&observation.primary_reason),
            "blocked_requirements": blocked_requirements,
        }),
    );
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "loop_stop",
            "reason": "contract_observation_incomplete",
            "contract_enforcement": options.contract_enforcement_label(),
            "session_scope": options.scope.as_str(),
            "step_kind": options.step_kind.map(RunSessionStepKind::as_str).unwrap_or(""),
            "phase_scope": options.phase_scope.as_deref().unwrap_or(""),
            "verify_attempts": verify_attempts,
            "missing_paths": observation.missing_paths.clone(),
            "missing_capabilities": observation.missing_capabilities.clone(),
            "missing_evidence": observation.missing_evidence.clone(),
            "missing_obligations": observation.missing_obligations.clone(),
            "primary_reason": eval_events::body_snippet(&observation.primary_reason),
        }),
    );
}

fn reachability_action_labels(reachability: &RepairReachability) -> Vec<&'static str> {
    reachability
        .viable_actions
        .iter()
        .map(|action| action.as_str())
        .collect()
}

fn emit_empty_response_escalation(
    config: &Config,
    options: &RunSessionOptions,
    stage: &str,
    attempt: usize,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "empty_response_escalation",
            "stage": stage,
            "attempt": attempt,
            "session_scope": options.scope.as_str(),
            "step_kind": options.step_kind.map(RunSessionStepKind::as_str).unwrap_or(""),
            "phase_scope": options.phase_scope.as_deref().unwrap_or(""),
        }),
    );
}

fn extract_elapsed_ms(message: &str) -> Option<u64> {
    message.lines().find_map(|line| {
        let value = line.trim().strip_prefix("elapsed_ms:")?.trim();
        value.parse::<u64>().ok()
    })
}

fn emit_provider_turn_timeout(
    config: &Config,
    options: &RunSessionOptions,
    attempt: usize,
    terminal: bool,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "provider_turn_timeout",
            "classification": "provider_turn_timeout",
            "attempt": attempt,
            "retry_limit": 1,
            "terminal": terminal,
            "timeout_secs": config.chat_timeout_secs,
            "next_action": if terminal { "terminal_handoff" } else { "retry_once" },
            "session_scope": options.scope.as_str(),
            "step_kind": options.step_kind.map(RunSessionStepKind::as_str).unwrap_or(""),
            "phase_scope": options.phase_scope.as_deref().unwrap_or(""),
        }),
    );
}

#[allow(clippy::too_many_arguments)]
fn stop_for_provider_turn_timeout(
    config: &Config,
    user_prompt: &str,
    options: &RunSessionOptions,
    changed_paths: &[String],
    verify_attempts: usize,
    tool_calls: usize,
    last_blocking_reason: Option<String>,
    timeout_count: usize,
    elapsed: Duration,
) -> anyhow::Result<RunSessionOutcome> {
    let failure_kind = "provider_turn_timeout";
    let recovery_paths =
        save_provider_turn_timeout_handoff(config, user_prompt, options, changed_paths);
    let elapsed_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "loop_stop",
            "reason": failure_kind,
            "missing_paths": [],
            "verify_attempts": verify_attempts,
            "tool_calls": tool_calls,
            "provider_turn_timeout_count": timeout_count,
            "provider_turn_elapsed_ms": elapsed_ms,
            "provider_turn_timeout_secs": config.chat_timeout_secs,
            "last_blocking_reason": last_blocking_reason,
            "last_provider_error": failure_kind,
            "session_scope": options.scope.as_str(),
            "step_kind": options.step_kind.map(RunSessionStepKind::as_str).unwrap_or(""),
            "phase_scope": options.phase_scope.as_deref().unwrap_or(""),
            "recovery_prompt_path": recovery_paths
                .as_ref()
                .map(|paths| paths.prompt_path.as_str())
                .unwrap_or(""),
            "recovery_ultra_plan_path": recovery_paths
                .as_ref()
                .map(|paths| paths.yaml_path.as_str())
                .unwrap_or(""),
            "recovery_yaml_missing": recovery_paths.is_none(),
        }),
    );
    bail!(render_minimal_recovery_stop_reason(
        format!(
            "{failure_kind}: provider turn exceeded the configured wall-clock cap after one retry (timeout_secs={}, elapsed_ms={elapsed_ms})",
            config.chat_timeout_secs
        ),
        recovery_paths.as_ref(),
    ))
}

fn save_provider_turn_timeout_handoff(
    config: &Config,
    user_prompt: &str,
    options: &RunSessionOptions,
    changed_paths: &[String],
) -> Option<MinimalRecoveryPaths> {
    let failure_kind = "provider_turn_timeout";
    let profile = if config.profile.trim().is_empty() {
        "generic"
    } else {
        config.profile.as_str()
    };
    let failed_phase = options
        .phase_scope
        .clone()
        .or_else(|| Some("minimal-loop".to_string()));
    let failed_step = options
        .step_kind
        .map(|kind| kind.as_str().to_string())
        .or_else(|| Some("model-call".to_string()));
    let handoff = crate::planner::repair::RecoveryHandoff {
        profile: profile.to_string(),
        original_goal: user_prompt.to_string(),
        failed_phase,
        failed_step,
        failure_kind: failure_kind.to_string(),
        failure_evidence: vec![
            format!(
                "provider_turn_timeout: provider exceeded configured wall-clock cap of {}s twice",
                config.chat_timeout_secs
            ),
            "The run terminated honestly instead of requiring human interruption.".to_string(),
        ],
        missing_paths: Vec::new(),
        missing_capabilities: Vec::new(),
        verify_commands: Vec::new(),
        changed_paths: changed_paths.to_vec(),
        repair_targets: vec!["retry_with_bounded_provider_turns".to_string()],
    };
    let prompt_path = match crate::planner::repair::save_ultra_recovery_prompt(
        &config.workspace_root,
        "provider-turn-timeout",
        &handoff,
    ) {
        Ok(path) => path,
        Err(err) => {
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "recovery_prompt_save_failed",
                    "recovery_handoff_kind": failure_kind,
                    "reason": eval_events::body_snippet(&err.to_string()),
                    "status": "incomplete",
                }),
            );
            return None;
        }
    };
    let yaml_path = match crate::planner::repair::save_recovery_ultra_plan(
        &config.workspace_root,
        "provider-turn-timeout",
        &handoff,
    ) {
        Ok(path) => path,
        Err(err) => {
            let prompt_display =
                crate::planner::repair::workspace_relative_handoff_path(&prompt_path);
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "recovery_ultra_plan_save_failed",
                    "recovery_handoff_kind": failure_kind,
                    "recovery_prompt_path": prompt_display,
                    "reason": eval_events::body_snippet(&err.to_string()),
                    "recovery_yaml_missing": true,
                    "status": "incomplete",
                }),
            );
            return None;
        }
    };
    let suggested_prompt_command =
        crate::planner::repair::suggested_ultra_recovery_command(&prompt_path, profile);
    let suggested_yaml_command =
        crate::planner::repair::suggested_recovery_ultra_plan_command(&yaml_path);
    let prompt_display = crate::planner::repair::workspace_relative_handoff_path(&prompt_path);
    let yaml_display = crate::planner::repair::workspace_relative_handoff_path(&yaml_path);
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "recovery_prompt_saved",
            "recovery_handoff_kind": failure_kind,
            "recovery_prompt_path": &prompt_display,
            "recovery_ultra_plan_path": &yaml_display,
            "recovery_yaml_missing": false,
            "recovery_yaml_roundtrip_ok": true,
            "suggested_recovery_command": suggested_prompt_command,
            "suggested_recovery_yaml_command": suggested_yaml_command,
            "recovery_profile": profile,
            "local_repair_exhausted": true,
            "failure_kind": failure_kind,
            "status": "incomplete",
        }),
    );
    Some(MinimalRecoveryPaths {
        prompt_path: prompt_display,
        yaml_path: yaml_display,
        suggested_prompt_command,
        suggested_yaml_command,
    })
}

#[allow(clippy::too_many_arguments)]
fn stop_for_model_empty_response(
    config: &Config,
    user_prompt: &str,
    options: &RunSessionOptions,
    changed_paths: &[String],
    verify_attempts: usize,
    tool_calls: usize,
    last_blocking_reason: Option<String>,
    last_provider_error: Option<String>,
    empty_response_count: usize,
) -> anyhow::Result<RunSessionOutcome> {
    let failure_kind = "model_empty_response";
    let recovery_paths = save_model_empty_response_handoff(
        config,
        user_prompt,
        options,
        changed_paths,
        empty_response_count,
    );
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "loop_stop",
            "reason": failure_kind,
            "missing_paths": [],
            "verify_attempts": verify_attempts,
            "tool_calls": tool_calls,
            "empty_response_count": empty_response_count,
            "last_blocking_reason": last_blocking_reason,
            "last_provider_error": last_provider_error.as_deref().map(eval_events::body_snippet),
            "session_scope": options.scope.as_str(),
            "step_kind": options.step_kind.map(RunSessionStepKind::as_str).unwrap_or(""),
            "phase_scope": options.phase_scope.as_deref().unwrap_or(""),
            "recovery_prompt_path": recovery_paths
                .as_ref()
                .map(|paths| paths.prompt_path.as_str())
                .unwrap_or(""),
            "recovery_ultra_plan_path": recovery_paths
                .as_ref()
                .map(|paths| paths.yaml_path.as_str())
                .unwrap_or(""),
            "recovery_yaml_missing": recovery_paths.is_none(),
        }),
    );
    bail!(render_minimal_recovery_stop_reason(
        format!(
            "{failure_kind}: assistant returned empty responses after bounded recovery ({empty_response_count})"
        ),
        recovery_paths.as_ref(),
    ))
}

fn save_model_empty_response_handoff(
    config: &Config,
    user_prompt: &str,
    options: &RunSessionOptions,
    changed_paths: &[String],
    empty_response_count: usize,
) -> Option<MinimalRecoveryPaths> {
    let failure_kind = "model_empty_response";
    let profile = if config.profile.trim().is_empty() {
        "generic"
    } else {
        config.profile.as_str()
    };
    let failed_phase = options
        .phase_scope
        .clone()
        .or_else(|| Some("minimal-loop".to_string()));
    let failed_step = options
        .step_kind
        .map(|kind| kind.as_str().to_string())
        .or_else(|| Some("model-call".to_string()));
    let handoff = crate::planner::repair::RecoveryHandoff {
        profile: profile.to_string(),
        original_goal: user_prompt.to_string(),
        failed_phase,
        failed_step,
        failure_kind: failure_kind.to_string(),
        failure_evidence: vec![
            "model_empty_response: assistant returned empty responses after two nudges and one fresh-session retry".to_string(),
            format!("empty_response_count: {empty_response_count}"),
        ],
        missing_paths: Vec::new(),
        missing_capabilities: Vec::new(),
        verify_commands: Vec::new(),
        changed_paths: changed_paths.to_vec(),
        repair_targets: vec!["resume_step_with_tool_calls".to_string()],
    };
    let prompt_path = match crate::planner::repair::save_ultra_recovery_prompt(
        &config.workspace_root,
        "model-empty-response",
        &handoff,
    ) {
        Ok(path) => path,
        Err(err) => {
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "recovery_prompt_save_failed",
                    "recovery_handoff_kind": failure_kind,
                    "reason": eval_events::body_snippet(&err.to_string()),
                    "status": "incomplete",
                }),
            );
            return None;
        }
    };
    let yaml_path = match crate::planner::repair::save_recovery_ultra_plan(
        &config.workspace_root,
        "model-empty-response",
        &handoff,
    ) {
        Ok(path) => path,
        Err(err) => {
            let prompt_display =
                crate::planner::repair::workspace_relative_handoff_path(&prompt_path);
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "recovery_ultra_plan_save_failed",
                    "recovery_handoff_kind": failure_kind,
                    "recovery_prompt_path": prompt_display,
                    "reason": eval_events::body_snippet(&err.to_string()),
                    "recovery_yaml_missing": true,
                    "status": "incomplete",
                }),
            );
            return None;
        }
    };
    let suggested_prompt_command =
        crate::planner::repair::suggested_ultra_recovery_command(&prompt_path, profile);
    let suggested_yaml_command =
        crate::planner::repair::suggested_recovery_ultra_plan_command(&yaml_path);
    let prompt_display = crate::planner::repair::workspace_relative_handoff_path(&prompt_path);
    let yaml_display = crate::planner::repair::workspace_relative_handoff_path(&yaml_path);
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "recovery_prompt_saved",
            "recovery_handoff_kind": failure_kind,
            "recovery_prompt_path": &prompt_display,
            "recovery_ultra_plan_path": &yaml_display,
            "recovery_yaml_missing": false,
            "recovery_yaml_roundtrip_ok": true,
            "suggested_recovery_command": suggested_prompt_command,
            "suggested_recovery_yaml_command": suggested_yaml_command,
            "recovery_profile": profile,
            "local_repair_exhausted": true,
            "failure_kind": failure_kind,
            "status": "incomplete",
        }),
    );
    Some(MinimalRecoveryPaths {
        prompt_path: prompt_display,
        yaml_path: yaml_display,
        suggested_prompt_command,
        suggested_yaml_command,
    })
}

#[derive(Debug, Clone)]
struct MinimalRecoveryPaths {
    prompt_path: String,
    yaml_path: String,
    suggested_prompt_command: String,
    suggested_yaml_command: String,
}

fn render_minimal_recovery_stop_reason(
    free_text: impl Into<String>,
    recovery_paths: Option<&MinimalRecoveryPaths>,
) -> String {
    let mut parts = eval_events::StopReasonParts::free_text(free_text);
    if let Some(paths) = recovery_paths {
        parts
            .paths
            .push(format!("recovery prompt saved: {}", paths.prompt_path));
        parts
            .paths
            .push(format!("recovery YAML saved: {}", paths.yaml_path));
        parts.commands.push(format!(
            "suggested command: {}",
            paths.suggested_prompt_command
        ));
        parts.commands.push(format!(
            "suggested YAML command: {}",
            paths.suggested_yaml_command
        ));
    }
    eval_events::render_stop_reason(&parts)
}

#[allow(clippy::too_many_arguments)]
fn save_minimal_recovery_handoff(
    root: &Path,
    eval_events_path: Option<&Path>,
    contract: &CompletionContract,
    goal: &str,
    failure_kind: &str,
    primary_reason: &str,
    report: &crate::planner::verify::VerificationReport,
    changed_paths: &[String],
    repair_target: RepairTarget,
) -> Option<MinimalRecoveryPaths> {
    let profile = contract
        .profile
        .clone()
        .unwrap_or_else(|| "generic".to_string());
    let original_goal = contract
        .goal
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| goal.to_string());
    let failure_evidence = std::iter::once(primary_reason.to_string())
        .chain(report.dependency_missing.iter().cloned())
        .chain(
            report
                .command_failures
                .iter()
                .map(|failure| format!("{}: {}", failure.command, failure.reason)),
        )
        .chain(
            report
                .verifier_command_false_negatives
                .iter()
                .map(|failure| {
                    format!(
                        "deterministic_verify_command_bug: {}: {}",
                        failure.command, failure.reason
                    )
                }),
        )
        .chain(
            report
                .compile_errors
                .iter()
                .map(|error| format!("implementation_compile_error: {}", error.summary())),
        )
        .chain(report.profile_failures.iter().cloned())
        .collect::<Vec<_>>();
    let handoff = crate::planner::repair::RecoveryHandoff {
        profile: profile.clone(),
        original_goal,
        failed_phase: Some("minimal-loop".to_string()),
        failed_step: Some("completion-verify".to_string()),
        failure_kind: failure_kind.to_string(),
        failure_evidence,
        missing_paths: report.missing_paths.clone(),
        missing_capabilities: contract.required_capabilities.clone(),
        verify_commands: contract.verify_commands.clone(),
        changed_paths: changed_paths.to_vec(),
        repair_targets: vec![repair_target.as_str().to_string()],
    };
    let prompt_path =
        match crate::planner::repair::save_ultra_recovery_prompt(root, "minimal-loop", &handoff) {
            Ok(path) => path,
            Err(err) => {
                eval_events::emit(
                    eval_events_path,
                    json!({
                        "event": "recovery_prompt_save_failed",
                        "recovery_handoff_kind": failure_kind,
                        "reason": eval_events::body_snippet(&err.to_string()),
                        "status": "incomplete",
                    }),
                );
                return None;
            }
        };
    let yaml_path =
        match crate::planner::repair::save_recovery_ultra_plan(root, "minimal-loop", &handoff) {
            Ok(path) => path,
            Err(err) => {
                let prompt_display =
                    crate::planner::repair::workspace_relative_handoff_path(&prompt_path);
                eval_events::emit(
                    eval_events_path,
                    json!({
                        "event": "recovery_ultra_plan_save_failed",
                        "recovery_handoff_kind": failure_kind,
                        "recovery_prompt_path": prompt_display,
                        "reason": eval_events::body_snippet(&err.to_string()),
                        "recovery_yaml_missing": true,
                        "status": "incomplete",
                    }),
                );
                return None;
            }
        };
    let suggested_prompt_command =
        crate::planner::repair::suggested_ultra_recovery_command(&prompt_path, &profile);
    let suggested_yaml_command =
        crate::planner::repair::suggested_recovery_ultra_plan_command(&yaml_path);
    let prompt_display = crate::planner::repair::workspace_relative_handoff_path(&prompt_path);
    let yaml_display = crate::planner::repair::workspace_relative_handoff_path(&yaml_path);
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "recovery_prompt_saved",
            "recovery_handoff_kind": failure_kind,
            "recovery_prompt_path": &prompt_display,
            "recovery_ultra_plan_path": &yaml_display,
            "recovery_yaml_missing": false,
            "recovery_yaml_roundtrip_ok": true,
            "suggested_recovery_command": suggested_prompt_command,
            "suggested_recovery_yaml_command": suggested_yaml_command,
            "recovery_profile": profile,
            "local_repair_exhausted": true,
            "failure_kind": failure_kind,
            "status": "incomplete",
        }),
    );
    Some(MinimalRecoveryPaths {
        prompt_path: prompt_display,
        yaml_path: yaml_display,
        suggested_prompt_command,
        suggested_yaml_command,
    })
}

fn dependency_setup_status(lifecycles: &[BuildVerifierLifecycleObservation]) -> &'static str {
    if lifecycles.is_empty() {
        return "not_required";
    }
    if lifecycles
        .iter()
        .any(|lifecycle| lifecycle.setup_status() == "passed")
    {
        return "ready";
    }
    if lifecycles
        .iter()
        .any(|lifecycle| matches!(lifecycle.setup_status(), "failed" | "timed_out"))
    {
        return "failed";
    }
    if lifecycles
        .iter()
        .any(|lifecycle| lifecycle.setup_status() == "blocked")
    {
        return "blocked";
    }
    if lifecycles
        .iter()
        .any(|lifecycle| lifecycle.final_status == BuildVerifierStatus::DependencyMissing)
    {
        return "missing";
    }
    if lifecycles
        .iter()
        .any(|lifecycle| lifecycle.final_status == BuildVerifierStatus::PolicyRejected)
    {
        return "policy_rejected";
    }
    if lifecycles
        .iter()
        .all(|lifecycle| lifecycle.final_status == BuildVerifierStatus::Passed)
    {
        return "ready";
    }
    "blocked"
}

fn handle_verify_repair_no_edit(
    root: &Path,
    eval_events_path: Option<&Path>,
    contract: &CompletionContract,
    goal: &str,
    state: &mut VerifyRepairState,
    changed_paths: &[String],
    options: &RunSessionOptions,
) -> anyhow::Result<VerifyRepairNoEditOutcome> {
    let Some(signature) = state.pending_signature.as_ref() else {
        return Ok(VerifyRepairNoEditOutcome::NoPendingFailure);
    };
    state.no_edit_turns += 1;
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "verify_repair_turn",
            "has_edit": false,
            "inspect_only": state.no_edit_turns == 1,
            "failure_signature": signature.label(),
            "repair_target": state.pending_target.map(RepairTarget::as_str).unwrap_or(""),
            "no_edit_turns": state.no_edit_turns,
        }),
    );
    if state.no_edit_turns >= VERIFY_REPAIR_NO_EDIT_LIMIT {
        let repair_target = state.pending_target.unwrap_or(RepairTarget::Unknown);
        let report = crate::planner::verify::VerificationReport::profile_failed(format!(
            "verify_repair_no_change: no file changes after verifier failure {}",
            signature.label()
        ));
        if options.contract_enforcement == ContractEnforcement::Observe {
            let observation = ContractObservation {
                missing_paths: Vec::new(),
                missing_capabilities: state.pending_error_context.missing_capabilities.clone(),
                missing_evidence: state.pending_error_context.missing_evidence.clone(),
                missing_obligations: state.pending_error_context.missing_obligations.clone(),
                primary_reason: report.primary_reason(),
            };
            emit_contract_observation_incomplete(
                eval_events_path,
                options,
                contract,
                &observation,
                state.no_edit_turns,
                "verify_repair_no_change_observed",
                &[],
            );
            eval_events::emit(
                eval_events_path,
                json!({
                    "event": "loop_stop",
                    "reason": "verify_repair_no_change_observed",
                    "failure_signature": signature.label(),
                    "contract_enforcement": options.contract_enforcement_label(),
                    "session_scope": options.scope.as_str(),
                    "step_kind": options.step_kind.map(RunSessionStepKind::as_str).unwrap_or(""),
                    "phase_scope": options.phase_scope.as_deref().unwrap_or(""),
                    "repair_target": state.pending_target.map(RepairTarget::as_str).unwrap_or(""),
                    "repair_target_followed": false,
                    "target_relation": "no_change",
                    "repair_follow_through": "no_change",
                    "changed_paths_before": state.changed_paths_at_failure.clone(),
                    "changed_paths_after": changed_paths,
                    "repair_turn_changed_paths": Vec::<String>::new(),
                    "missing_capabilities": observation.missing_capabilities.clone(),
                    "missing_evidence": observation.missing_evidence.clone(),
                    "missing_obligations": observation.missing_obligations.clone(),
                    "recovery_prompt_path": "",
                    "recovery_ultra_plan_path": "",
                    "recovery_yaml_missing": true,
                    "no_edit_turns": state.no_edit_turns,
                }),
            );
            return Ok(VerifyRepairNoEditOutcome::ObservationIncomplete(
                observation,
            ));
        }
        let recovery_paths = save_minimal_recovery_handoff(
            root,
            eval_events_path,
            contract,
            goal,
            "verify_repair_no_change",
            &report.primary_reason(),
            &report,
            changed_paths,
            repair_target,
        );
        eval_events::emit(
            eval_events_path,
            json!({
                "event": "loop_stop",
                "reason": "verify_repair_no_change",
                "failure_signature": signature.label(),
                "repair_target": state.pending_target.map(RepairTarget::as_str).unwrap_or(""),
                "repair_target_followed": false,
                "target_relation": "no_change",
                "repair_follow_through": "no_change",
                "changed_paths_before": state.changed_paths_at_failure.clone(),
                "changed_paths_after": changed_paths,
                "repair_turn_changed_paths": Vec::<String>::new(),
                "recovery_prompt_path": recovery_paths
                    .as_ref()
                    .map(|paths| paths.prompt_path.clone())
                    .unwrap_or_default(),
                "recovery_ultra_plan_path": recovery_paths
                    .as_ref()
                    .map(|paths| paths.yaml_path.clone())
                    .unwrap_or_default(),
                "recovery_yaml_missing": recovery_paths.is_none(),
                "no_edit_turns": state.no_edit_turns,
            }),
        );
        let mut error_context = state.pending_error_context.clone();
        if error_context.is_empty() {
            error_context = RunSessionErrorContext::from_repair_target(repair_target);
        } else if error_context.repair_target.is_none() {
            error_context.repair_target = Some(repair_target.as_str().to_string());
        }
        return Err(RunSessionError::new(
            render_minimal_recovery_stop_reason(
                "completion contract verify repair made no file changes",
                recovery_paths.as_ref(),
            ),
            error_context,
        )
        .into());
    }
    Ok(VerifyRepairNoEditOutcome::Feedback(
        super::feedback::verify_repair_edit_required(
            &signature.label(),
            state.no_edit_turns,
            VERIFY_REPAIR_NO_EDIT_LIMIT,
        ),
    ))
}

fn reanchored_verify_feedback_if_needed(
    feedback: String,
    repair_follow_through: Option<RepairFollowThrough>,
    repair_target: RepairTarget,
    report: &crate::planner::verify::VerificationReport,
    contract: &CompletionContract,
    runtime_acceptance: &RuntimeAcceptanceReport,
) -> String {
    if !matches!(
        repair_follow_through,
        Some(RepairFollowThrough::TargetNotFollowed | RepairFollowThrough::UnrelatedChange)
    ) {
        return feedback;
    }
    let paths = follow_through_anchor_paths(repair_target, report, contract, runtime_acceptance);
    format!(
        "Previous edit did not address the failure. You must edit one of the following files: {}\n\n{}",
        paths.join(", "),
        feedback
    )
}

fn follow_through_anchor_paths(
    repair_target: RepairTarget,
    report: &crate::planner::verify::VerificationReport,
    contract: &CompletionContract,
    runtime_acceptance: &RuntimeAcceptanceReport,
) -> Vec<String> {
    if !report.compile_errors.is_empty() {
        return report
            .compile_errors
            .iter()
            .map(|error| error.path.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
    }
    let mut paths = Vec::new();
    for target in &runtime_acceptance.obligation_repair_targets {
        if target.target_role == "implementation" && !target.target_path.trim().is_empty() {
            push_unique(&mut paths, target.target_path.clone());
        }
    }
    if !paths.is_empty() {
        return paths;
    }
    if contract
        .required_obligations
        .iter()
        .any(|role| role == "implementation")
    {
        for path in target_implementation_files(report, Some(contract)) {
            push_unique(&mut paths, path);
        }
    }
    if !paths.is_empty() {
        return paths;
    }
    if matches!(
        repair_target,
        RepairTarget::CapabilityMissing | RepairTarget::EmptyApp | RepairTarget::Implementation
    ) {
        for path in target_implementation_files(report, Some(contract)) {
            push_unique(&mut paths, path);
        }
    }
    if !paths.is_empty() {
        return paths;
    }
    for path in target_implementation_files(report, Some(contract)) {
        push_unique(&mut paths, path);
    }
    if paths.is_empty() {
        paths.push("src/app/page.tsx".to_string());
    }
    paths
}

fn push_unique(out: &mut Vec<String>, value: String) {
    if !out.contains(&value) {
        out.push(value);
    }
}

fn terminal_verify_stop_reason(
    report: &crate::planner::verify::VerificationReport,
    signature: &VerificationSignature,
    previous_signature: Option<&VerificationSignature>,
    verdict: RepairProgressVerdict,
    repair_target: RepairTarget,
) -> String {
    if !report.compile_errors.is_empty() {
        return "implementation_compile_error".to_string();
    }
    if !report.verifier_command_false_negatives.is_empty() {
        return "deterministic_verify_command_bug".to_string();
    }
    if !report.dependency_missing.is_empty() {
        return "dependency_setup_missing".to_string();
    }
    if report.command_failures.iter().any(|failure| {
        failure.reason.contains("build_verify_policy_rejected")
            || failure.reason.contains("verify command may not")
    }) {
        return "verify_command_policy_error".to_string();
    }
    if report.command_failures.iter().any(|failure| {
        failure.reason.contains("build_verify_failed")
            || failure.command.contains("npm run build")
            || failure.command.contains("next build")
    }) {
        return "build_verify_failed".to_string();
    }
    if report
        .profile_failures
        .iter()
        .any(|reason| reason.contains("build_verify_blocked"))
    {
        return "build_verify_blocked".to_string();
    }
    if report
        .profile_failures
        .iter()
        .any(|reason| reason.contains("missing_required_capabilities"))
    {
        return "missing_required_capabilities".to_string();
    }
    if report
        .profile_failures
        .iter()
        .any(|reason| reason.contains("missing_required_evidence"))
    {
        return "missing_required_evidence".to_string();
    }
    if report
        .profile_failures
        .iter()
        .any(|reason| reason.contains("weak_verification_evidence"))
    {
        return "weak_verification_evidence".to_string();
    }
    if report
        .profile_failures
        .iter()
        .any(|reason| reason.contains("tailwind_contract_failure"))
    {
        return "tailwind_contract_failure".to_string();
    }
    if report
        .profile_failures
        .iter()
        .any(|reason| !reason.contains("deferred verify requirement pending"))
    {
        return "profile_contract_failure".to_string();
    }
    if report
        .profile_failures
        .iter()
        .any(|reason| reason.contains("deferred verify requirement pending"))
    {
        return "deferred_verify_requirement_pending".to_string();
    }
    if signature.has_test_discovery_failure() {
        return "test_discovery_failure".to_string();
    }
    if signature.has_test_framework_mismatch() {
        return "test_framework_mismatch".to_string();
    }
    if previous_signature.is_some() {
        match verdict {
            RepairProgressVerdict::Unchanged
            | RepairProgressVerdict::Regressed
            | RepairProgressVerdict::Invalid => {
                if repair_target == RepairTarget::DependencySetup {
                    return "dependency_setup_blocked".to_string();
                }
                return format!("verify_repair_progress_{}", verdict.as_str());
            }
            RepairProgressVerdict::Passed | RepairProgressVerdict::Improved => {}
        }
    }
    "verify_repair_exhausted".to_string()
}

fn should_emit_artifact_recovery(
    enabled: bool,
    non_edit_streak: usize,
    missing_paths: &[String],
    contract: Option<&CompletionContract>,
    root: &Path,
) -> bool {
    enabled
        && !missing_paths.is_empty()
        && non_edit_streak >= ARTIFACT_NON_EDIT_STAGNATION_THRESHOLD
        && !contract.is_some_and(|contract| contract.dependency_precondition_active(root))
}

fn implement_step(options: &RunSessionOptions) -> bool {
    options.step_kind == Some(RunSessionStepKind::Implement)
}

fn read_only_objective_excerpt(user_prompt: &str) -> String {
    eval_events::body_snippet(user_prompt)
}

fn maybe_read_only_stagnation_feedback(
    eval_events_path: Option<&Path>,
    user_prompt: &str,
    read_only_streak: usize,
    options: &RunSessionOptions,
) -> Option<String> {
    let stage = match read_only_streak {
        READ_ONLY_STAGNATION_INTERVENTION_THRESHOLD => "intervention",
        READ_ONLY_STAGNATION_COMPACT_THRESHOLD => "compact_restatement",
        _ => return None,
    };
    let objective = read_only_objective_excerpt(user_prompt);
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "read_only_stagnation_feedback",
            "stage": stage,
            "read_only_streak": read_only_streak,
            "objective": objective,
            "session_scope": options.scope.as_str(),
            "step_kind": options.step_kind.map(RunSessionStepKind::as_str).unwrap_or(""),
            "phase_scope": options.phase_scope.as_deref().unwrap_or(""),
        }),
    );
    Some(if stage == "compact_restatement" {
        super::feedback::read_only_stagnation_compact(&objective, read_only_streak)
    } else {
        super::feedback::read_only_stagnation(&objective, read_only_streak)
    })
}

struct ArtifactRecoveryFeedbackContext<'a> {
    eval_events_path: Option<&'a Path>,
    enabled: bool,
    missing_paths: &'a [String],
    required_paths: &'a [String],
    contract: Option<&'a CompletionContract>,
    root: &'a Path,
}

fn maybe_artifact_recovery_feedback(
    state: &mut ArtifactRecoveryState,
    non_edit_streak: &mut usize,
    context: ArtifactRecoveryFeedbackContext<'_>,
) -> anyhow::Result<Option<String>> {
    if !should_emit_artifact_recovery(
        context.enabled,
        *non_edit_streak,
        context.missing_paths,
        context.contract,
        context.root,
    ) {
        return Ok(None);
    }
    let target_path = state
        .sync_target(context.required_paths, context.missing_paths)
        .unwrap_or_default();
    state.target_attempts += 1;
    if state.target_attempts > ARTIFACT_RECOVERY_ATTEMPT_LIMIT {
        eval_events::emit(
            context.eval_events_path,
            json!({
                "event": "loop_stop",
                "reason": "artifact_recovery_exhausted",
                "missing_paths": context.missing_paths,
                "non_edit_streak": *non_edit_streak,
                "attempts": state.target_attempts - 1,
                "last_target_path": target_path,
                "last_model_action": state.last_model_action,
            }),
        );
        bail!("artifact recovery exhausted");
    }
    eval_events::emit(
        context.eval_events_path,
        json!({
            "event": "artifact_stagnation_feedback",
            "missing_paths": context.missing_paths,
            "attempt": state.target_attempts,
            "attempt_limit": ARTIFACT_RECOVERY_ATTEMPT_LIMIT,
            "non_edit_streak": *non_edit_streak,
            "target_path": target_path,
            "target_attempt": state.target_attempts,
            "last_model_action": state.last_model_action,
        }),
    );
    *non_edit_streak = 0;
    Ok(Some(super::feedback::artifact_stagnation_for_target(
        context.missing_paths,
        &target_path,
        state.target_attempts,
        ARTIFACT_RECOVERY_ATTEMPT_LIMIT,
    )))
}

fn provider_error_allows_xml_fallback(err: &anyhow::Error) -> bool {
    let lower = err.to_string().to_ascii_lowercase();
    if lower.contains(" api failed:")
        || lower.contains("http")
        || lower.contains("status")
        || lower.contains("network")
        || lower.contains("timeout")
    {
        return false;
    }
    lower.contains("function_call")
        || lower.contains("tool call")
        || lower.contains("tool_call")
        || lower.contains("provider parse")
        || lower.contains("parse")
}

fn provider_error_allows_native_tool_retry(err: &anyhow::Error) -> bool {
    let lower = err.to_string().to_ascii_lowercase();
    if lower.contains(" api failed:")
        || lower.contains("http")
        || lower.contains("status")
        || lower.contains("network")
        || lower.contains("timeout")
    {
        return false;
    }
    lower.contains("function_call")
        || lower.contains("tool call")
        || lower.contains("tool_call")
        || lower.contains("provider parse")
}

pub(crate) fn extract_requested_artifact_paths(root: &Path, prompt: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    let mut in_required_block = false;
    for line in prompt.lines() {
        let trimmed = line.trim();
        if trimmed
            .to_ascii_lowercase()
            .starts_with("required final artifacts")
        {
            in_required_block = true;
            continue;
        }
        if in_required_block {
            if trimmed.is_empty() {
                continue;
            }
            if !is_artifact_list_line(trimmed) && looks_like_section_boundary(trimmed) {
                in_required_block = false;
            } else if let Some(candidate) = artifact_candidate_from_line(trimmed)
                && requested_artifact_path_allowed(root, &candidate)
                && seen.insert(candidate.clone())
            {
                paths.push(candidate);
                continue;
            }
        }
        for candidate in backticked_candidates(trimmed) {
            if looks_like_artifact_path(&candidate)
                && requested_artifact_path_allowed(root, &candidate)
                && seen.insert(candidate.clone())
            {
                paths.push(candidate);
            }
        }
    }
    paths
}

fn requested_artifact_path_allowed(root: &Path, raw: &str) -> bool {
    if validate_workspace_relative(raw).is_err() {
        return false;
    }
    let path = Path::new(raw);
    let blocked = [".anvil", ".git", "target", "node_modules", ".next"];
    if path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| blocked.contains(&part))
    }) {
        return false;
    }
    resolve_optional_existing(root, raw).is_ok()
}

fn required_paths_satisfied_after_tool(
    root: &Path,
    required_paths: &[String],
    initially_missing_paths: &[String],
    write_or_edit_seen: bool,
) -> bool {
    if required_paths.is_empty() || !missing_paths(root, required_paths).is_empty() {
        return false;
    }
    write_or_edit_seen
        || initially_missing_paths
            .iter()
            .any(|path| resolve_existing(root, path).is_ok())
}

fn is_artifact_list_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("- ")
        || line.starts_with("* ")
        || line.chars().next().is_some_and(|ch| ch.is_ascii_digit())
}

fn looks_like_section_boundary(line: &str) -> bool {
    line.ends_with(':') || line.starts_with('#')
}

fn artifact_candidate_from_line(line: &str) -> Option<String> {
    let mut value = line.trim();
    value = value
        .trim_start_matches("- ")
        .trim_start_matches("* ")
        .trim_start();
    if let Some((head, tail)) = value.split_once(". ")
        && head.chars().all(|ch| ch.is_ascii_digit())
    {
        value = tail.trim_start();
    }
    let first = value.split_whitespace().next().unwrap_or_default();
    let candidate = first
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('\'')
        .trim_end_matches([',', ';']);
    if looks_like_artifact_path(candidate) {
        Some(candidate.to_string())
    } else {
        None
    }
}

fn backticked_candidates(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('`') else {
            break;
        };
        out.push(after_start[..end].trim().to_string());
        rest = &after_start[end + 1..];
    }
    out
}

fn looks_like_artifact_path(value: &str) -> bool {
    if value.is_empty() || value.starts_with("http://") || value.starts_with("https://") {
        return false;
    }
    if value.contains('/') {
        return true;
    }
    matches!(
        value,
        "Cargo.toml"
            | "README.md"
            | "package.json"
            | "tsconfig.json"
            | "index.html"
            | "pyproject.toml"
    ) || Path::new(value).extension().is_some_and(|ext| {
        matches!(
            ext.to_str().unwrap_or_default(),
            "js" | "jsx"
                | "ts"
                | "tsx"
                | "rs"
                | "py"
                | "md"
                | "txt"
                | "csv"
                | "json"
                | "toml"
                | "yaml"
                | "yml"
                | "html"
                | "css"
        )
    })
}

fn looks_like_progress_without_tool(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains("i will")
        || lower.contains("next")
        || lower.contains("作成します")
        || lower.contains("実装します")
        || lower.contains("進めます")
}

fn looks_like_action_prompt(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains("create")
        || lower.contains("write")
        || lower.contains("edit")
        || lower.contains("fix")
        || lower.contains("implement")
        || lower.contains("add ")
        || lower.contains("build")
        || lower.contains("作成")
        || lower.contains("実装")
        || lower.contains("修正")
        || lower.contains("追加")
}

fn recoverable_tool_feedback(
    name: &str,
    err: &anyhow::Error,
    edit_anchor_failure: Option<(String, usize)>,
) -> String {
    let err_text = err.to_string();
    if err_text.contains("verify_command_policy_error") {
        return format!(
            "Tool call `{name}` was rejected by deterministic verify policy: {err_text}. Allowed alternatives: use one bounded verifier command such as `npm run build`, `cargo test`, `python -m compileall -q src`, or `test -f relative/path`; create files with the Write tool; keep verify to one deterministic command; python-cli behavior-probe fixture CSVs already exist when required; move dependency installation or dev-server startup to setup/runtime phases, not verify."
        );
    }
    if err_text.contains("stale_absolute_path_recoverable") {
        return format!(
            "Tool call `{name}` used an absolute path outside the current workspace and was rejected: {err_text}. Retry with the workspace-relative path named in the error; do not use an absolute path from another workspace."
        );
    }
    if let Some((path, count)) = edit_anchor_failure {
        let mut feedback = format!(
            "Tool call `{name}` was rejected with a recoverable validation error: {err_text}. Retry with the Edit tool using the deterministic best-match excerpt and re-anchor mandate from the error."
        );
        if count >= 2 {
            feedback = format!(
                "Tool call `{name}` was rejected with a recoverable validation error: {err_text}. This is edit anchor failure #{count} for `{path}` in this step; switch to the Write tool and write the complete corrected file content for that one file."
            );
        }
        return feedback;
    }
    format!(
        "Tool call `{name}` was rejected with a recoverable validation error: {err}. Retry with the same tool or another available tool using a valid JSON object that matches the tool schema."
    )
}

fn edit_anchor_tracking_path(root: &Path, arguments: &serde_json::Value) -> Option<String> {
    let raw = arguments.get("path")?.as_str()?;
    let path = resolve_optional_existing(root, raw).ok()?;
    let root = root.canonicalize().ok()?;
    Some(crate::tools::path_guard::relative_display(&root, &path))
}

fn changed_path_from_call(root: &Path, arguments: &serde_json::Value) -> Option<String> {
    let raw = arguments.get("path")?.as_str()?;
    let path = resolve_existing(root, raw).ok()?;
    let root = root.canonicalize().ok()?;
    Some(crate::tools::path_guard::relative_display(&root, &path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::verify::VerificationReport;
    use crate::providers::AssistantReply;
    use crate::state::ToolCall;
    use serde_json::json;

    struct Fake {
        replies: Vec<anyhow::Result<AssistantReply>>,
    }

    impl ChatClient for Fake {
        fn label(&self) -> &str {
            "fake"
        }
        fn supports_native_tools(&self, _model: &str) -> bool {
            true
        }
        fn chat(
            &mut self,
            _model: &str,
            _messages: &[ConversationMessage],
            _tools: &[crate::tools::registry::ToolSpec],
            _native_tools_enabled: bool,
        ) -> anyhow::Result<AssistantReply> {
            self.replies.remove(0)
        }
    }

    struct DelayedFake {
        replies: Vec<(Duration, anyhow::Result<AssistantReply>)>,
    }

    impl ChatClient for DelayedFake {
        fn label(&self) -> &str {
            "delayed-fake"
        }
        fn supports_native_tools(&self, _model: &str) -> bool {
            true
        }
        fn chat(
            &mut self,
            _model: &str,
            _messages: &[ConversationMessage],
            _tools: &[crate::tools::registry::ToolSpec],
            _native_tools_enabled: bool,
        ) -> anyhow::Result<AssistantReply> {
            let (delay, reply) = self.replies.remove(0);
            std::thread::sleep(delay);
            reply
        }
    }

    struct RecordingFake {
        replies: Vec<anyhow::Result<AssistantReply>>,
        requests: Vec<Vec<ConversationMessage>>,
    }

    impl ChatClient for RecordingFake {
        fn label(&self) -> &str {
            "recording-fake"
        }
        fn supports_native_tools(&self, _model: &str) -> bool {
            true
        }
        fn chat(
            &mut self,
            _model: &str,
            messages: &[ConversationMessage],
            _tools: &[crate::tools::registry::ToolSpec],
            _native_tools_enabled: bool,
        ) -> anyhow::Result<AssistantReply> {
            self.requests.push(messages.to_vec());
            self.replies.remove(0)
        }
    }

    fn empty_reply() -> AssistantReply {
        AssistantReply {
            content: String::new(),
            tool_calls: Vec::new(),
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    fn read_reply(path: &str) -> AssistantReply {
        AssistantReply {
            content: String::new(),
            tool_calls: vec![ToolCall::new("Read", json!({"path": path}))],
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    fn config(root: std::path::PathBuf) -> Config {
        Config {
            workspace_root: root,
            state_dir: std::path::PathBuf::from("state"),
            yes: true,
            offline: false,
            context_budget: 1000,
            model: "m".to_string(),
            provider: crate::config::Provider::Ollama,
            prompt_layout: crate::config::PromptLayout::Stable,
            planner_model: "m".to_string(),
            planner_provider: crate::config::Provider::Ollama,
            ollama_host: "http://localhost:11434".to_string(),
            num_predict: 100,
            max_iterations: 4,
            chat_timeout_secs: 1,
            chat_timeout_source: "override:test".to_string(),
            field_sources: crate::config::ConfigFieldSources::default(),
            chat_retries: 1,
            eval_events_path: None,
            completion_contract_path: None,
            resume: None,
            fresh_session: false,
            no_footer: false,
            narration: crate::config::NarrationMode::Normal,
            profile: "generic".to_string(),
            profile_explicit: false,
            profile_inference: None,
            style: "default".to_string(),
            action: crate::config::Action::Repl,
        }
    }

    fn event_values(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect()
    }

    #[test]
    fn command_timeout_repetition_uses_similarity_and_guides_strategy_change() {
        let mut state = RecoverableToolErrorState::default();
        let first = anyhow::anyhow!(
            "command_timeout: ls -R && cat package.json\nstatus: timed out\nelapsed_ms: 180001"
        );
        let second = anyhow::anyhow!(
            "command_timeout: ls -R src/app && cat package.json\nstatus: timed out\nelapsed_ms: 180013"
        );
        let third =
            anyhow::anyhow!("command_timeout: LS    -R\nstatus: timed out\nelapsed_ms: 180021");

        assert_eq!(state.record("Bash", &first), 1);
        assert_eq!(state.record("Bash", &second), 2);
        assert_eq!(state.record("Bash", &third), 3);
        assert_eq!(command_timeout_similarity_key(&first.to_string()), "ls -R");
        let feedback = command_timeout_strategy_feedback(&first.to_string(), 2);
        assert!(feedback.contains("ls -R recurses"), "{feedback}");
        assert!(feedback.contains("node_modules"), "{feedback}");
        assert!(feedback.contains("list src/"), "{feedback}");
    }

    #[test]
    fn step_wall_clock_cap_self_terminates_with_time_sink() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.md"), "facts").unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 8;
        let mut fake = DelayedFake {
            replies: vec![(Duration::from_millis(5), Ok(read_reply("notes.md")))],
        };
        let mut session = SessionSnapshot::new();
        let mut options = RunSessionOptions::plan_step(RunSessionStepKind::Implement);
        options.step_wall_clock_cap = Some(Duration::from_millis(1));

        let err = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Implement the requested helper after inspecting notes.md.",
            &[],
            &cfg,
            &NOOP_UI,
            options,
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("step_wall_clock_exhausted"), "{err}");
        assert!(err.contains("provider"), "{err}");
        let events = event_values(&events);
        let stop = events
            .iter()
            .find(|event| event.get("event").and_then(Value::as_str) == Some("loop_stop"))
            .unwrap();
        assert_eq!(
            stop.get("reason").and_then(Value::as_str),
            Some("step_wall_clock_exhausted")
        );
        assert!(
            stop.get("dominant_time_sink")
                .and_then(Value::as_str)
                .unwrap()
                .contains("provider"),
            "{stop}"
        );
    }

    #[test]
    fn fake_write_then_final() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.txt","content":"ok"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session_with_required_paths(
            &mut fake,
            &mut session,
            "create a.txt",
            &["a.txt".to_string()],
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "required artifacts satisfied: a.txt");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "ok"
        );
    }

    #[test]
    fn repeated_empty_responses_stop_as_model_empty_response_with_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 1;
        let mut fake = Fake {
            replies: vec![
                Ok(empty_reply()),
                Ok(empty_reply()),
                Ok(empty_reply()),
                Ok(empty_reply()),
            ],
        };
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "Create the content editor setup artifacts.",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("model_empty_response"), "{err}");
        assert!(err.contains("recovery prompt saved:"), "{err}");
        let events = event_values(&events);
        let stages = events
            .iter()
            .filter(|event| {
                event.get("event").and_then(Value::as_str) == Some("empty_response_escalation")
            })
            .map(|event| {
                event
                    .get("stage")
                    .and_then(Value::as_str)
                    .unwrap()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            stages,
            vec![
                "nudge_1",
                "nudge_2",
                "fresh_session_retry_scheduled",
                "fresh_session_retry"
            ]
        );
        let stop = events
            .iter()
            .find(|event| event.get("event").and_then(Value::as_str) == Some("loop_stop"))
            .unwrap();
        assert_eq!(
            stop.get("reason").and_then(Value::as_str),
            Some("model_empty_response")
        );
        let recovery = events
            .iter()
            .find(|event| {
                event.get("event").and_then(Value::as_str) == Some("recovery_prompt_saved")
            })
            .unwrap();
        let prompt_path = recovery
            .get("recovery_prompt_path")
            .and_then(Value::as_str)
            .unwrap();
        let prompt = std::fs::read_to_string(dir.path().join(prompt_path)).unwrap();
        assert!(prompt.contains("model_empty_response"), "{prompt}");
    }

    #[test]
    fn provider_turn_timeout_retries_once_then_stops_with_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.chat_timeout_secs = 0;
        let mut fake = DelayedFake {
            replies: vec![
                (
                    Duration::from_millis(1),
                    Ok(AssistantReply::text("discarded first reply")),
                ),
                (
                    Duration::from_millis(1),
                    Ok(AssistantReply::text("discarded second reply")),
                ),
            ],
        };
        let mut session = SessionSnapshot::new();

        let err = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "Create a bounded provider turn artifact.",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("provider_turn_timeout"), "{err}");
        assert!(err.contains("recovery prompt saved:"), "{err}");
        let events = event_values(&events);
        let durations = events
            .iter()
            .filter(|event| {
                event.get("event").and_then(Value::as_str) == Some("provider_turn_duration")
            })
            .collect::<Vec<_>>();
        assert_eq!(durations.len(), 2, "{events:?}");
        assert!(durations.iter().all(|event| {
            event
                .get("timed_out")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        }));
        let timeouts = events
            .iter()
            .filter(|event| {
                event.get("event").and_then(Value::as_str) == Some("provider_turn_timeout")
            })
            .collect::<Vec<_>>();
        assert_eq!(timeouts.len(), 2, "{events:?}");
        assert_eq!(
            timeouts[0].get("terminal").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            timeouts[1].get("terminal").and_then(Value::as_bool),
            Some(true)
        );
        let stop = events
            .iter()
            .find(|event| event.get("event").and_then(Value::as_str) == Some("loop_stop"))
            .unwrap();
        assert_eq!(
            stop.get("reason").and_then(Value::as_str),
            Some("provider_turn_timeout")
        );
        let recovery = events
            .iter()
            .find(|event| {
                event.get("event").and_then(Value::as_str) == Some("recovery_prompt_saved")
            })
            .unwrap();
        let prompt_path = recovery
            .get("recovery_prompt_path")
            .and_then(Value::as_str)
            .unwrap();
        let prompt = std::fs::read_to_string(dir.path().join(prompt_path)).unwrap();
        assert!(prompt.contains("provider_turn_timeout"), "{prompt}");
    }

    #[test]
    fn empty_response_recovery_at_second_nudge_continues_normally() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 2;
        let mut fake = Fake {
            replies: vec![
                Ok(empty_reply()),
                Ok(empty_reply()),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.txt","content":"ok"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "Create the requested file.",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap();
        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
        assert!(dir.path().join("a.txt").is_file());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"stage\":\"nudge_1\""));
        assert!(event_text.contains("\"stage\":\"nudge_2\""));
        assert!(event_text.contains("\"event\":\"empty_response_recovered\""));
        assert!(!event_text.contains("model_empty_response"));
    }

    #[test]
    fn empty_response_fresh_session_retry_uses_step_only_context_and_continues() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 2;
        let mut fake = RecordingFake {
            replies: vec![
                Ok(empty_reply()),
                Ok(empty_reply()),
                Ok(empty_reply()),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.txt","content":"ok"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
            ],
            requests: Vec::new(),
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "Create the requested file with a tool call.",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap();
        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
        assert_eq!(fake.requests.len(), 5);
        let fresh_request = &fake.requests[3];
        assert_eq!(
            fresh_request
                .iter()
                .filter(|message| message.role == "user"
                    && message.content == "Create the requested file with a tool call.")
                .count(),
            1
        );
        assert!(
            fresh_request
                .iter()
                .all(|message| message.role != "assistant" && message.role != "tool"),
            "{fresh_request:?}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"stage\":\"fresh_session_retry\""));
        assert!(event_text.contains("\"fresh_session_retry\":true"));
        assert!(!event_text.contains("model_empty_response"));
    }

    #[test]
    fn default_run_session_options_preserve_prompt_artifact_extraction() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"a.txt","content":"ok"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "Create the file.\n\nRequired final artifacts:\n- a.txt",
            &[],
            &config(dir.path().to_path_buf()),
            &NOOP_UI,
        )
        .unwrap();
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert!(dir.path().join("a.txt").is_file());
    }

    #[test]
    fn plan_step_disables_prompt_required_artifact_extraction() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply::text("workspace inspected"))],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Inspect workspace.\n\nRequired final artifacts:\n- README.md",
            &[],
            &config(dir.path().to_path_buf()),
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Inspect),
        )
        .unwrap();
        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
        assert!(!dir.path().join("README.md").exists());
    }

    #[test]
    fn plan_step_disables_completion_contract_verification_side_effects() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(&contract, "not json").unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply::text("workspace inspected"))],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Inspect workspace",
            &[],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Inspect),
        )
        .unwrap();
        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
    }

    #[test]
    fn implement_plan_step_keeps_completion_contract_authority_enabled() {
        let implement = RunSessionOptions::plan_step(RunSessionStepKind::Implement);
        assert!(implement.contract_runtime_enabled());
        assert!(implement.contract_path_merge_enabled());

        for kind in [
            RunSessionStepKind::Inspect,
            RunSessionStepKind::Setup,
            RunSessionStepKind::Verify,
            RunSessionStepKind::Report,
        ] {
            let options = RunSessionOptions::plan_step(kind);
            assert!(!options.contract_runtime_enabled(), "{kind:?}");
            assert!(!options.contract_path_merge_enabled(), "{kind:?}");
        }
    }

    #[test]
    fn plan_step_interactive_completion_requires_capability_evidence_after_path_exists() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let static_page = "export default function Page(){ return <main><canvas /></main>; }";
        let interactive_page = r#""use client";
import { useEffect, useState } from "react";
export default function Page(){
  const [score,setScore] = useState(0);
  const [gameState,setGameState] = useState("playing");
  const enemies = [{ x: 1, y: 2 }];
  useEffect(() => {
    const onKeyDown = () => setScore((value) => value + 1);
    const frame = requestAnimationFrame(() => {
      const collision = enemies.some((enemy) => enemy.x > 0);
      if (collision) {
        setScore((value) => value + 10);
        setGameState("gameover");
      }
    });
    window.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);
  return <main tabIndex={0} onKeyDown={() => setScore(score + 1)}><canvas /><p>enemy collision score {score} {gameState}</p></main>;
}
"#;
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"src/app/page.tsx","content":static_page}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"src/app/page.tsx","content":interactive_page}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let prompt = "Execute exactly one StepPlan step.\n\nCurrent step kind:\nimplement\n\nRequired final capabilities:\n- stateful_interaction\n- player_control\n- adversary_or_challenge\n- progression_or_score\n- failure_or_collision_rule\n\nRequired final evidence:\n- implementation_artifact\n- visible_interactive_surface_evidence\n- user_input_handler_evidence\n- stateful_update_evidence\n- challenge_or_adversary_evidence\n- score_or_progression_evidence\n- failure_or_collision_evidence\n\nExpected paths after this step:\n- src/app/page.tsx";
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            prompt,
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Implement),
        )
        .unwrap();
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert_eq!(outcome.iterations, 2);
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"step_capability_evidence_check\""));
        assert!(event_text.contains("\"ok\":false"));
        assert!(event_text.contains("\"ok\":true"));
        assert!(event_text.contains("stateful_update_evidence"));
        assert!(event_text.contains("failure_or_collision_evidence"));
    }

    #[test]
    fn plan_step_non_interactive_completion_preserves_path_only_success() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"a.txt","content":"ok"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Execute one implement step.\n\nExpected paths after this step:\n- a.txt",
            &["a.txt".to_string()],
            &config(dir.path().to_path_buf()),
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Implement),
        )
        .unwrap();
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert_eq!(outcome.iterations, 1);
    }

    #[test]
    fn verify_step_with_bash_then_final_text_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Bash", json!({"command":"true"}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("verification passed")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Create app. Verify the current step.",
            &[],
            &config(dir.path().to_path_buf()),
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Verify),
        )
        .unwrap();
        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
        assert_eq!(outcome.tool_calls, 1);
    }

    #[test]
    fn verify_step_with_bash_only_can_delegate_to_runner() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new("Bash", json!({"command":"true"}))],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Create app. Verify the current step.",
            &[],
            &config(dir.path().to_path_buf()),
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Verify),
        )
        .unwrap();
        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
        assert_eq!(outcome.final_text, "step tool observation completed");
        assert_eq!(outcome.tool_calls, 1);
    }

    #[test]
    fn verify_step_bash_semantic_pipe_is_policy_error_not_evidence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"scripts":{}}"#).unwrap();
        let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events_path.clone());
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Bash",
                        json!({"command":"cat package.json | grep 3011"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Bash",
                        json!({"command":"test -f package.json"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Create app. Verify the current step.",
            &[],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Verify),
        )
        .unwrap();
        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
        assert_eq!(outcome.final_text, "step tool observation completed");
        let events = event_values(&events_path);
        let policy_error = events
            .iter()
            .find(|event| event.get("event").and_then(Value::as_str) == Some("tool_policy_error"));
        assert_eq!(
            policy_error.and_then(|event| event.get("policy_error_kind")),
            Some(&json!("verify_command_policy_error"))
        );
        assert_eq!(
            policy_error.and_then(|event| event.get("verify_command_violation_kind")),
            Some(&json!("shell_control_syntax"))
        );
        let blocked_policy = events.iter().find(|event| {
            event.get("event").and_then(Value::as_str) == Some("runtime_bash_policy")
                && event.get("blocked").and_then(Value::as_bool) == Some(true)
        });
        assert_eq!(
            blocked_policy.and_then(|event| event.get("deterministic_verifier_evidence")),
            Some(&json!(false))
        );
        let successful_bash_execs = events
            .iter()
            .filter(|event| {
                event.get("event").and_then(Value::as_str) == Some("tool_execute")
                    && event.get("name").and_then(Value::as_str) == Some("Bash")
                    && event.get("status").and_then(Value::as_str) == Some("ok")
            })
            .count();
        assert_eq!(successful_bash_execs, 1);
    }

    #[test]
    fn verify_step_runtime_splits_shell_control_bash_segments() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page() {}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"next build"}}"#,
        )
        .unwrap();
        let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events_path.clone());
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Bash",
                    json!({"command":"ls -R src/app && node -p \"require('./package.json').scripts.build\""}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Create app. Verify the current step.",
            &[],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Verify),
        )
        .unwrap();
        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
        let bash_result = session
            .messages
            .iter()
            .find(|message| message.role == "tool" && message.name.as_deref() == Some("Bash"))
            .map(|message| message.content.as_str())
            .unwrap_or_default();
        assert!(
            bash_result.contains("combined_outcome: Success"),
            "{bash_result}"
        );
        assert!(
            bash_result.contains("segment 1 command: ls -R src/app"),
            "{bash_result}"
        );
        assert!(
            bash_result.contains("segment 2 command: node -p"),
            "{bash_result}"
        );
        assert!(bash_result.contains("page.tsx"), "{bash_result}");
        assert!(bash_result.contains("next build"), "{bash_result}");
        let events = event_values(&events_path);
        assert!(
            !events.iter().any(
                |event| event.get("event").and_then(Value::as_str) == Some("tool_policy_error")
            ),
            "{events:?}"
        );
        let policy = events
            .iter()
            .find(|event| event.get("event").and_then(Value::as_str) == Some("runtime_bash_policy"))
            .unwrap();
        assert_eq!(
            policy.get("normalization_kind").and_then(Value::as_str),
            Some("shell_control_split")
        );
    }

    #[test]
    fn verify_step_runtime_substitutes_install_segment_before_verify_segment() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"node -e \"process.exit(0)\""},"dependencies":{}}"#,
        )
        .unwrap();
        let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events_path.clone());
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Bash",
                    json!({"command":"npm install && test -f package.json"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Verify the current step.",
            &[],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Verify)
                .with_dependency_setup_authority(NodeDependencySetupAuthority::PlanSetupStep),
        )
        .unwrap();

        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
        let bash_result = session
            .messages
            .iter()
            .find(|message| message.role == "tool" && message.name.as_deref() == Some("Bash"))
            .map(|message| message.content.as_str())
            .unwrap_or_default();
        assert!(
            bash_result.contains("segment 1 install substituted: npm install"),
            "{bash_result}"
        );
        assert!(
            bash_result.contains("segment 2 command: test -f package.json"),
            "{bash_result}"
        );
        let events = event_values(&events_path);
        assert!(events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("verify_install_substituted")
                && event.get("trigger").and_then(Value::as_str) == Some("verify_segment")
        }));
    }

    #[test]
    fn verify_step_runtime_splits_multiline_bash_segments() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        std::fs::write(dir.path().join("src/main.py"), "print('ok')\n").unwrap();
        let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events_path.clone());
        cfg.max_iterations = 2;
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Bash",
                        json!({"command":"test -f package.json\npython -m compileall -q src"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Verify the current step.",
            &[],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Verify),
        )
        .unwrap();

        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
        let bash_result = session
            .messages
            .iter()
            .find(|message| message.role == "tool" && message.name.as_deref() == Some("Bash"))
            .map(|message| message.content.as_str())
            .unwrap_or_default();
        assert!(
            bash_result.contains("combined_outcome: Success"),
            "{bash_result}"
        );
        assert!(
            bash_result.contains("segment 2 command: python -m compileall -q src"),
            "{bash_result}"
        );
        let events = event_values(&events_path);
        let policy = events
            .iter()
            .find(|event| event.get("event").and_then(Value::as_str) == Some("runtime_bash_policy"))
            .unwrap();
        assert_eq!(
            policy.get("normalization_kind").and_then(Value::as_str),
            Some("shell_control_split")
        );
    }

    #[test]
    fn verify_step_redirect_rejection_feedback_names_write_tool_remedy() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events_path);
        cfg.max_iterations = 2;
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Bash",
                        json!({"command":"cat > fixtures/input.csv\npython src/main.py fixtures/input.csv"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Verify the current step.",
            &[],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Verify),
        )
        .unwrap();

        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
        let feedback = session
            .messages
            .iter()
            .find(|message| message.role == "tool" && message.name.as_deref() == Some("Bash"))
            .map(|message| message.content.as_str())
            .unwrap_or_default();
        assert!(
            feedback.contains("create files with the Write tool"),
            "{feedback}"
        );
        assert!(
            feedback.contains("keep verify to one deterministic command"),
            "{feedback}"
        );
        assert!(
            feedback.contains("python-cli behavior-probe fixture CSVs already exist"),
            "{feedback}"
        );
    }

    #[test]
    fn verify_step_runtime_preserves_and_short_circuit() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new("Bash", json!({"command":"false && echo x"}))],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Create app. Verify the current step.",
            &[],
            &config(dir.path().to_path_buf()),
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Verify),
        )
        .unwrap();
        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
        let bash_result = session
            .messages
            .iter()
            .find(|message| message.role == "tool" && message.name.as_deref() == Some("Bash"))
            .map(|message| message.content.as_str())
            .unwrap_or_default();
        assert!(
            bash_result.contains("combined_outcome: CommandFailed"),
            "{bash_result}"
        );
        assert!(
            bash_result.contains("segment 2 skipped by && short-circuit: echo x"),
            "{bash_result}"
        );
        assert!(!bash_result.contains("stdout:\nx"), "{bash_result}");
    }

    #[test]
    fn verify_step_runtime_strips_output_truncation_pipe_before_execute() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events_path.clone());
        let command = format!(
            "cd {} && test -f missing-marker.txt 2>&1 | tail -80",
            dir.path().display()
        );
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new("Bash", json!({"command": command}))],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Create app. Verify the current step.",
            &[],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Verify),
        )
        .unwrap();
        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
        assert_eq!(outcome.final_text, "step tool observation completed");
        let bash_result = session
            .messages
            .iter()
            .find(|message| message.role == "tool" && message.name.as_deref() == Some("Bash"))
            .map(|message| message.content.as_str())
            .unwrap_or_default();
        assert!(
            bash_result.contains("outcome: CommandFailed"),
            "{bash_result}"
        );
        assert!(
            bash_result.contains("command did not succeed: test -f missing-marker.txt"),
            "{bash_result}"
        );
        assert!(!bash_result.contains("tail -80"), "{bash_result}");
        let events = event_values(&events_path);
        let policy = events
            .iter()
            .find(|event| event.get("event").and_then(Value::as_str) == Some("runtime_bash_policy"))
            .unwrap();
        assert_eq!(
            policy.get("normalization_kind").and_then(Value::as_str),
            Some("workspace_cd_normalized")
        );
        assert_eq!(policy.get("blocked").and_then(Value::as_bool), Some(false));
        let normalization = events.iter().find(|event| {
            event.get("event").and_then(Value::as_str)
                == Some("verify_command_normalized_at_runtime")
        });
        assert_eq!(
            normalization.and_then(|event| event.get("kind")),
            Some(&json!("workspace_cd_normalized"))
        );
        assert!(
            normalization
                .and_then(|event| event.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains("mask the base command exit status"),
            "{events:?}"
        );
        assert!(
            normalization
                .and_then(|event| event.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains("workspace cd"),
            "{events:?}"
        );
    }

    #[test]
    fn verify_step_runtime_strips_custom_status_echo_shell_control_before_policy_bail() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page() {}\n",
        )
        .unwrap();
        let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events_path.clone());
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Bash",
                    json!({"command":"test -f src/app/page.tsx && echo \"EXISTS\" || echo \"MISSING\""}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Create app. Verify the current step.",
            &[],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Verify),
        )
        .unwrap();
        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
        assert_eq!(outcome.final_text, "step tool observation completed");
        let bash_result = session
            .messages
            .iter()
            .find(|message| message.role == "tool" && message.name.as_deref() == Some("Bash"))
            .map(|message| message.content.as_str())
            .unwrap_or_default();
        assert!(bash_result.contains("outcome: Success"), "{bash_result}");
        assert!(bash_result.contains("command succeeded"), "{bash_result}");
        assert!(!bash_result.contains("echo \"EXISTS\""), "{bash_result}");
        let events = event_values(&events_path);
        assert!(
            !events.iter().any(
                |event| event.get("event").and_then(Value::as_str) == Some("tool_policy_error")
            ),
            "{events:?}"
        );
        let policy = events
            .iter()
            .find(|event| event.get("event").and_then(Value::as_str) == Some("runtime_bash_policy"))
            .unwrap();
        assert_eq!(
            policy.get("normalization_kind").and_then(Value::as_str),
            Some("success_failure_echo_stripped")
        );
        assert_eq!(policy.get("blocked").and_then(Value::as_bool), Some(false));
        let normalization = events.iter().find(|event| {
            event.get("event").and_then(Value::as_str)
                == Some("verify_command_normalized_at_runtime")
        });
        assert_eq!(
            normalization.and_then(|event| event.get("repaired").and_then(Value::as_str)),
            Some("test -f src/app/page.tsx")
        );
    }

    #[test]
    fn verify_command_tool_boundary_inventory_routes_through_shared_normalizer() {
        let source = include_str!("loop_run.rs");
        let implementation = source.split("#[cfg(test)]").next().unwrap_or(source);
        let execute_sites = implementation
            .match_indices("registry.execute_with_cancel(")
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert_eq!(
            execute_sites,
            vec![
                implementation
                    .find("registry.execute_with_cancel(")
                    .unwrap()
            ],
            "new tool execution boundaries must be allowlisted and routed through runtime_bash_policy_decision"
        );
        let execute_index = execute_sites[0];
        let policy_call = implementation[..execute_index]
            .rfind("runtime_bash_policy_decision(")
            .expect("Bash policy decision before tool execution");
        let policy_impl = implementation
            .split("fn recovered_bash_command(")
            .next()
            .unwrap_or(implementation);
        policy_impl
            .find("normalize_runtime_bash_command_for_boundary(")
            .expect("runtime Bash policy routes through the shared normalizer");
        policy_impl
            .find("RuntimeBashPolicyDecision::for_step(")
            .expect("runtime Bash policy call constructs decisions through the normalized path");
        let normalized_set = implementation[policy_call..execute_index]
            .find("set_bash_command(&mut call.arguments, normalized_command)")
            .expect("normalized verifier command is written back before execution");
        implementation[policy_call..execute_index]
            .find("execute_split_runtime_bash(")
            .expect("runtime Bash shell-control split executes through bounded segments");
        let substitute_set = implementation[policy_call..execute_index]
            .find("set_bash_command(&mut call.arguments, substitute)")
            .expect("repetition substitution is written back before repeated-error bail");
        assert!(normalized_set > substitute_set || substitute_set > normalized_set);
    }

    #[test]
    fn setup_step_bash_shell_control_is_runtime_setup_not_verifier_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events_path.clone());
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Bash",
                    json!({"command":"printf ok && printf done"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Set up the current step.",
            &[],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Setup),
        )
        .unwrap();
        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
        let events = event_values(&events_path);
        let policy = events.iter().find(|event| {
            event.get("event").and_then(Value::as_str) == Some("runtime_bash_policy")
        });
        assert_eq!(
            policy.and_then(|event| event.get("bash_policy_purpose")),
            Some(&json!("runtime_setup"))
        );
        assert_eq!(
            policy.and_then(|event| event.get("verifier_policy_checked")),
            Some(&json!(false))
        );
        assert_eq!(
            policy.and_then(|event| event.get("blocked")),
            Some(&json!(false))
        );
        assert_eq!(
            policy.and_then(|event| event.get("deterministic_verifier_evidence")),
            Some(&json!(false))
        );
        assert!(events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("tool_execute")
                && event.get("name").and_then(Value::as_str) == Some("Bash")
                && event.get("status").and_then(Value::as_str) == Some("ok")
        }));
    }

    #[test]
    fn completion_contract_without_verify_preserves_early_success() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["a.txt"],"verify_commands":[]}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"a.txt","content":"ok"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create the file",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap();
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
    }

    #[test]
    fn minimal_loop_nextjs_required_paths_only_does_not_complete() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["package.json","src/app/page.tsx","src/app/layout.tsx","src/app/global.d.ts"],"verify_commands":[],"profile":"nextjs","goal":"Create a Next.js app","deferred_verify_requirements":[{"command":"npm run build","reason":"requires dependency setup","authority":"postcheck","profile":"nextjs","status":"blocked_by_dependency_setup"}],"verify_repair_cap":1}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    ToolCall::new("Write", json!({"path":"package.json","content":"{}"})),
                    ToolCall::new(
                        "Write",
                        json!({"path":"src/app/page.tsx","content":"export default function Page(){return <main/>;}\n"}),
                    ),
                    ToolCall::new(
                        "Write",
                        json!({"path":"src/app/layout.tsx","content":"export default function RootLayout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>}\n"}),
                    ),
                    ToolCall::new(
                        "Write",
                        json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";\n"}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create a Next.js app",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("completion contract verify failed"), "{err}");
        assert!(
            err.contains("/run-ultra-plan .anvil/plans/recovery-ultra-plan-minimal-loop-"),
            "{err}"
        );
        assert!(err.contains(".yaml"), "{err}");
        assert!(dir.path().join("package.json").is_file());
    }

    #[test]
    fn setup_scaffold_completion_finishes_missing_nextjs_configs_at_budget_gate() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 1;
        let required_paths = crate::planner::profiles::nextjs::setup_scaffold_paths(dir.path());
        let page = "export default function Page(){return <main>ready</main>;}\n";
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![
                        ToolCall::new("Write", json!({"path":"package.json","content":"{}"})),
                        ToolCall::new("Write", json!({"path":"tsconfig.json","content":"{}"})),
                        ToolCall::new(
                            "Write",
                            json!({"path":"src/app/layout.tsx","content":"import './globals.css';\nexport default function RootLayout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>}\n"}),
                        ),
                        ToolCall::new("Write", json!({"path":"src/app/page.tsx","content":page})),
                        ToolCall::new(
                            "Write",
                            json!({"path":"src/app/globals.css","content":"@tailwind base;\n@tailwind components;\n@tailwind utilities;\n"}),
                        ),
                        ToolCall::new(
                            "Write",
                            json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";\n"}),
                        ),
                    ],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(empty_reply()),
            ],
        };
        let mut session = SessionSnapshot::new();

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Scaffold the Next.js setup files.",
            &required_paths,
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Setup),
        )
        .unwrap();

        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap(),
            page
        );
        let postcss = std::fs::read_to_string(dir.path().join("postcss.config.js")).unwrap();
        assert!(postcss.contains("tailwindcss"), "{postcss}");
        assert!(postcss.contains("autoprefixer"), "{postcss}");
        assert!(dir.path().join("tailwind.config.ts").is_file());
        assert!(
            outcome
                .changed_paths
                .contains(&"postcss.config.js".to_string())
        );
        assert!(
            outcome
                .changed_paths
                .contains(&"tailwind.config.ts".to_string())
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"setup_scaffold_completed\""));
        assert!(event_text.contains("\"trigger\":\"budget_low\""));
        assert!(event_text.contains("postcss.config.js"));
        assert!(event_text.contains("tailwind.config.ts"));
    }

    #[test]
    fn setup_scaffold_completion_authors_absent_nextjs_application_page() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 1;
        let mut fake = Fake {
            replies: vec![
                Ok(empty_reply()),
                Ok(empty_reply()),
                Ok(empty_reply()),
                Ok(empty_reply()),
            ],
        };
        let mut session = SessionSnapshot::new();

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Scaffold the Next.js setup files.",
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Setup),
        )
        .unwrap();

        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert!(dir.path().join("src/app/page.tsx").exists());
        let page = std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap();
        assert!(page.contains("export default function Page"), "{page}");
        let event_text = std::fs::read_to_string(events).unwrap_or_default();
        assert!(event_text.contains("\"event\":\"setup_scaffold_completed\""));
        assert!(event_text.contains("\"trigger\":\"budget_low\""));
    }

    #[test]
    fn artifact_recovery_exhaustion_rescues_nextjs_setup_page_scaffold() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 8;
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply::text("I will create the page.")),
                Ok(AssistantReply::text("Still preparing.")),
                Ok(AssistantReply::text("I need to write it.")),
                Ok(AssistantReply::text("Scaffold now.")),
            ],
        };
        let mut session = SessionSnapshot::new();

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Scaffold the Next.js setup page for test0708_018.",
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Setup),
        )
        .unwrap();

        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert!(dir.path().join("src/app/page.tsx").is_file());
        assert!(
            outcome
                .changed_paths
                .contains(&"src/app/page.tsx".to_string())
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"artifact_stagnation_feedback\""));
        assert!(event_text.contains("\"event\":\"setup_scaffold_completed\""));
        assert!(event_text.contains("\"trigger\":\"exhausted\""));
        assert!(!event_text.contains("\"reason\":\"artifact_recovery_exhausted\""));
    }

    #[test]
    fn pre_scaffolded_setup_step_short_circuits_without_model_turn() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(dir.path().join("src/app/page.tsx"), "ok").unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut fake = Fake {
            replies: Vec::new(),
        };
        let mut session = SessionSnapshot::new();

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Scaffold the already present page.",
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Setup),
        )
        .unwrap();

        assert_eq!(fake.replies.len(), 0);
        assert_eq!(outcome.iterations, 0);
        assert_eq!(outcome.tool_calls, 0);
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        let events = event_values(&events);
        assert!(events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("step_short_circuited")
                && event.get("at").and_then(Value::as_str) == Some("start")
        }));
    }

    #[test]
    fn setup_step_with_missing_path_uses_normal_model_loop() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path": "src/app/page.tsx", "content": "ok"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Scaffold the missing page.",
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Setup),
        )
        .unwrap();

        assert_eq!(fake.replies.len(), 0);
        assert_eq!(outcome.iterations, 1);
        assert_eq!(outcome.tool_calls, 1);
        assert!(dir.path().join("src/app/page.tsx").is_file());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(!event_text.contains("\"event\":\"step_short_circuited\""));
    }

    #[test]
    fn stale_absolute_path_feedback_names_root_and_model_can_adapt() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({
                            "path": "/Users/example/share/work/old-run/src/app/layout.tsx",
                            "content": "wrong"
                        }),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path": "src/app/page.tsx", "content": "ok"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Create the page.",
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Setup),
        )
        .unwrap();

        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert_eq!(outcome.tool_calls, 2);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap(),
            "ok"
        );
        assert!(session.messages.iter().any(|message| {
            message.role == "tool"
                && message.content.contains("outside the current workspace")
                && message.content.contains("src/app/page.tsx")
        }));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"method\":\"root_anchor\""));
        assert!(event_text.contains("\"method\":\"required_path\""));
        assert!(event_text.contains("\"accepted\":false"));
        assert!(event_text.contains("\"normalized\":\"src/app/page.tsx\""));
    }

    #[test]
    fn stale_absolute_path_fallback_uses_current_plan_artifact_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({
                            "path": "/Users/example/share/work/old-run/src/app/page.tsx",
                            "content": "page"
                        }),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path": "package.json", "content": "{}"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let options = RunSessionOptions::plan_step(RunSessionStepKind::Setup)
            .with_path_fallback_candidates(vec!["src/app/page.tsx".to_string()]);

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Create package and page artifacts.",
            &["package.json".to_string()],
            &cfg,
            &NOOP_UI,
            options,
        )
        .unwrap();

        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap(),
            "page"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("package.json")).unwrap(),
            "{}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"method\":\"required_path\""));
        assert!(event_text.contains("\"accepted\":true"));
        assert!(event_text.contains("\"normalized\":\"src/app/page.tsx\""));
    }

    #[test]
    fn setup_scaffold_completion_finishes_python_cli_scaffold() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "python-cli".to_string();
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 1;
        let mut fake = Fake {
            replies: vec![Ok(empty_reply()), Ok(empty_reply())],
        };
        let mut session = SessionSnapshot::new();

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Scaffold the Python CLI setup files.",
            &[
                "pyproject.toml".to_string(),
                "src/csv_stats/main.py".to_string(),
            ],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Setup),
        )
        .unwrap();

        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert!(dir.path().join("pyproject.toml").is_file());
        assert!(dir.path().join("src/csv_stats/main.py").is_file());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"setup_scaffold_completed\""));
        assert!(event_text.contains("src/csv_stats/main.py"));
    }

    #[test]
    fn minimal_loop_repairs_after_completion_verify_failure() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["a.py"],"verify_commands":["python3 -m py_compile a.py"],"verify_repair_cap":2}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.py","content":"def broken(:\n    pass\n"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.py","content":"def fixed():\n    return 1\n"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create a.py",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap();
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::CompletionContractSatisfied
        );
        assert_eq!(outcome.verify_attempts, 2);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.py")).unwrap(),
            "def fixed():\n    return 1\n"
        );
        assert!(!session.messages.iter().any(|message| {
            message
                .content
                .contains("Deterministic completion verification failed")
        }));
    }

    #[test]
    fn minimal_loop_stops_with_verify_repair_exhausted_after_cap() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["a.py"],"verify_commands":["python3 -m py_compile a.py"],"verify_repair_cap":1}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"a.py","content":"def broken(:\n    pass\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create a.py",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("completion contract verify failed"));
    }

    #[test]
    fn artifact_stagnation_feedback_then_write_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["date-helper.js"],"verify_commands":[]}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Glob", json!({"pattern":"**/*"}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Grep", json!({"pattern":"date"}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Bash", json!({"command":"ls"}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"date-helper.js","content":"module.exports = {};\n"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create date-helper.js",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap();
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert!(dir.path().join("date-helper.js").is_file());
    }

    #[test]
    fn implement_read_only_stagnation_nudges_then_write_completes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.md"), "facts").unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 4;
        let mut fake = RecordingFake {
            replies: vec![
                Ok(read_reply("notes.md")),
                Ok(read_reply("notes.md")),
                Ok(read_reply("notes.md")),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"implemented.txt","content":"done\n"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
            ],
            requests: Vec::new(),
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Implement the requested helper after inspecting notes.md.",
            &[],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Implement),
        )
        .unwrap();

        assert_eq!(outcome.stop_reason, RunStopReason::AssistantFinal);
        assert!(dir.path().join("implemented.txt").is_file());
        let events = event_values(&events);
        let feedback = events
            .iter()
            .find(|event| {
                event.get("event").and_then(Value::as_str) == Some("read_only_stagnation_feedback")
            })
            .expect("read-only feedback event");
        assert_eq!(
            feedback.get("stage").and_then(Value::as_str),
            Some("intervention")
        );
        assert!(
            fake.requests.iter().any(|request| {
                request.iter().any(|message| {
                    message
                        .content
                        .contains("Inspection is sufficient - implement now via Write/Edit")
                })
            }),
            "{:#?}",
            fake.requests
        );
    }

    #[test]
    fn implement_read_only_stagnation_exhausts_with_honest_classification() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.md"), "facts").unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 2;
        let mut fake = Fake {
            replies: (0..5).map(|_| Ok(read_reply("notes.md"))).collect(),
        };
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Implement the requested helper after inspecting notes.md.",
            &[],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Implement),
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("model_stagnation:read_only_loop"), "{err}");
        assert!(err.contains("objective:"), "{err}");
        assert!(!err.contains("no concrete blocker recorded"), "{err}");
        let events = event_values(&events);
        let stages = events
            .iter()
            .filter(|event| {
                event.get("event").and_then(Value::as_str) == Some("read_only_stagnation_feedback")
            })
            .map(|event| {
                event
                    .get("stage")
                    .and_then(Value::as_str)
                    .unwrap()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert_eq!(stages, vec!["intervention", "compact_restatement"]);
        let stop = events
            .iter()
            .find(|event| event.get("event").and_then(Value::as_str) == Some("loop_stop"))
            .unwrap();
        assert_eq!(
            stop.get("reason").and_then(Value::as_str),
            Some("model_stagnation:read_only_loop")
        );
        assert_eq!(
            stop.get("read_only_streak").and_then(Value::as_u64),
            Some(5)
        );
    }

    #[test]
    fn artifact_recovery_exhausts_after_repeated_non_edit_tools() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["date-helper.js"],"verify_commands":[]}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        cfg.max_iterations = 13;
        let mut replies = Vec::new();
        for _ in 0..12 {
            replies.push(Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new("Glob", json!({"pattern":"**/*"}))],
                prompt_tokens: None,
                completion_tokens: None,
            }));
        }
        let mut fake = Fake { replies };
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create date-helper.js",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("artifact recovery exhausted"));
    }

    #[test]
    fn verify_failure_requires_edit_after_repeated_no_change() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        let events = dir.path().join("events.jsonl");
        std::fs::write(
            &contract,
            r#"{"required_paths":["a.py"],"verify_commands":["python3 -m py_compile a.py"],"verify_repair_cap":3}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 6;
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.py","content":"def broken(:\n    pass\n"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Read", json!({"path":"a.py"}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Bash",
                        json!({"command":"python3 -m py_compile a.py"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Read", json!({"path":"a.py"}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create a.py",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("verify repair made no file changes"));
        assert!(
            err.contains("/run-ultra-plan .anvil/plans/recovery-ultra-plan-minimal-loop-"),
            "{err}"
        );
        assert!(err.contains(".yaml"), "{err}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"reason\":\"verify_repair_no_change\""));
        assert!(event_text.contains("\"repair_follow_through\":\"no_change\""));
        assert!(event_text.contains("\"changed_paths_before\":[\"a.py\"]"));
        assert!(event_text.contains("\"changed_paths_after\":[\"a.py\"]"));
        assert!(event_text.contains("\"recovery_yaml_missing\":false"));
        assert!(dir.path().join(".anvil/repairs").is_dir());
        assert!(dir.path().join(".anvil/plans").is_dir());
    }

    #[test]
    fn observe_verify_repair_no_edit_returns_observation_without_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        let events = dir.path().join("events.jsonl");
        std::fs::write(
            &contract,
            r#"{"required_paths":["src/app/page.tsx"],"required_evidence":["challenge_or_adversary_evidence"],"verify_repair_cap":2}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 6;
        let static_page =
            "export default function Page(){ return <main><canvas>ready</canvas></main>; }";
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"src/app/page.tsx","content":static_page}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text(
                    "I inspected the page but made no edit.",
                )),
            ],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Create src/app/page.tsx",
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step_with_enforcement(
                RunSessionStepKind::Implement,
                ContractEnforcement::Observe,
                Some("phase-one".to_string()),
            ),
        )
        .unwrap();

        assert_eq!(
            outcome.stop_reason,
            RunStopReason::CompletionContractObservedIncomplete
        );
        assert_eq!(
            outcome.missing_evidence,
            vec!["challenge_or_adversary_evidence".to_string()]
        );
        assert!(!dir.path().join(".anvil/repairs").exists());
        let events = event_values(&events);
        assert!(events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("loop_stop")
                && event.get("reason").and_then(Value::as_str)
                    == Some("verify_repair_no_change_observed")
                && event.get("contract_enforcement").and_then(Value::as_str) == Some("observe")
                && event.get("phase_scope").and_then(Value::as_str) == Some("phase-one")
        }));
        assert!(events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("contract_observation_incomplete")
                && event
                    .get("missing_evidence")
                    .and_then(Value::as_array)
                    .is_some_and(|values| {
                        values
                            .iter()
                            .any(|value| value.as_str() == Some("challenge_or_adversary_evidence"))
                    })
        }));
    }

    #[test]
    fn enforce_verify_repair_no_edit_still_bails_and_saves_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        let events = dir.path().join("events.jsonl");
        std::fs::write(
            &contract,
            r#"{"required_paths":["src/app/page.tsx"],"required_evidence":["challenge_or_adversary_evidence"],"verify_repair_cap":2}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 6;
        let static_page =
            "export default function Page(){ return <main><canvas>ready</canvas></main>; }";
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"src/app/page.tsx","content":static_page}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text(
                    "I inspected the page but made no edit.",
                )),
            ],
        };
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Create src/app/page.tsx",
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step_with_enforcement(
                RunSessionStepKind::Implement,
                ContractEnforcement::Enforce,
                None,
            ),
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("verify repair made no file changes"), "{err}");
        assert!(dir.path().join(".anvil/repairs").is_dir());
        let events = event_values(&events);
        assert!(events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("loop_stop")
                && event.get("reason").and_then(Value::as_str) == Some("verify_repair_no_change")
                && event.get("recovery_yaml_missing").and_then(Value::as_bool) == Some(false)
        }));
        assert!(!events.iter().any(|event| {
            event.get("reason").and_then(Value::as_str) == Some("verify_repair_no_change_observed")
        }));
    }

    #[test]
    fn verify_repair_progress_unchanged_after_edit_still_exhausts() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["a.py"],"verify_commands":["python3 -m py_compile a.py"],"verify_repair_cap":2}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.py","content":"def broken(:\n    pass\n"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.py","content":"def broken(:\n    pass\n"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create a.py",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("completion contract verify failed"));
    }

    #[test]
    fn identical_failure_without_file_changes_stops_with_progress_reason() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        std::fs::write(dir.path().join("a.py"), "def broken(:\n    pass\n").unwrap();
        let contract = CompletionContract {
            required_paths: vec!["a.py".to_string()],
            verify_commands: vec!["python3 -m py_compile a.py".to_string()],
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
        let previous_report = contract.verify(dir.path());
        let previous_signature = VerificationSignature::from_report(&previous_report);
        let previous_target = classify_repair_target(&previous_report);
        let mut verify_attempts = 1;
        let err = verify_completion_contract(
            dir.path(),
            Some(&events),
            &contract,
            "fix a.py",
            &mut verify_attempts,
            Some(&previous_signature),
            Some(previous_target),
            &["a.py".to_string()],
            &["a.py".to_string()],
            &[],
            false,
            NodeDependencySetupAuthority::None,
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("completion contract verify failed"));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"reason\":\"verify_repair_progress_invalid\""));
        assert!(!event_text.contains("\"reason\":\"repair_target_not_followed\""));
        assert!(!event_text.contains("\"reason\":\"repair_unrelated_change\""));
    }

    #[test]
    fn target_not_followed_feedback_anchor_lists_contract_implementation_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/main.rs"),
            "fn main(){ assert_eq!(1, 1); }\n",
        )
        .unwrap();
        let events = dir.path().join("events.jsonl");
        let contract = CompletionContract {
            required_paths: vec!["src/main.rs".to_string()],
            verify_commands: Vec::new(),
            profile: None,
            goal: None,
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: vec![
                "test_artifact".to_string(),
                "non_zero_test_or_assertion_evidence".to_string(),
            ],
            evidence_hint_tokens: Vec::new(),
            required_obligations: vec!["implementation".to_string()],
            deferred_verify_requirements: Vec::new(),
            verify_repair_cap: 3,
        }
        .validate(dir.path())
        .unwrap();
        let mut previous_report = VerificationReport::pass();
        previous_report.push_profile_failure(
            "missing_required_evidence:test_artifact,non_zero_test_or_assertion_evidence",
        );
        let previous_signature = VerificationSignature::from_report(&previous_report);
        let previous_target = classify_repair_target(&previous_report);
        assert_eq!(previous_target, RepairTarget::RequiredEvidenceMissing);

        let mut verify_attempts = 1;
        let feedback = verify_completion_contract(
            dir.path(),
            Some(&events),
            &contract,
            "write a Rust program",
            &mut verify_attempts,
            Some(&previous_signature),
            Some(previous_target),
            &["src/main.rs".to_string()],
            &["src/main.rs".to_string()],
            &["src/main.rs".to_string()],
            true,
            NodeDependencySetupAuthority::None,
            false,
        )
        .unwrap()
        .expect("anchored feedback");
        assert!(feedback.feedback.starts_with(
            "Previous edit did not address the failure. You must edit one of the following files: src/main.rs"
        ));
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains("\"repair_follow_through\":\"target_not_followed\""));
        assert!(event_text.contains("\"failure_kind\":\"repair_target_not_followed\""));
        assert!(!event_text.contains("\"event\":\"loop_stop\""));
    }

    #[test]
    fn target_not_followed_then_correct_entrypoint_edit_can_verify_successfully() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        let events = dir.path().join("events.jsonl");
        let contract = CompletionContract {
            required_paths: vec!["src/app/page.tsx".to_string()],
            verify_commands: Vec::new(),
            profile: None,
            goal: None,
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: Vec::new(),
            evidence_hint_tokens: Vec::new(),
            required_obligations: Vec::new(),
            deferred_verify_requirements: Vec::new(),
            verify_repair_cap: 3,
        }
        .validate(dir.path())
        .unwrap();
        let previous_report = VerificationReport::missing_path("src/app/page.tsx");
        let previous_signature = VerificationSignature::from_report(&previous_report);
        let previous_target = classify_repair_target(&previous_report);
        assert_eq!(previous_target, RepairTarget::MissingEntrypoint);

        std::fs::write(
            dir.path().join("src/app/widget.tsx"),
            "export const Widget = null;\n",
        )
        .unwrap();
        let mut verify_attempts = 1;
        let feedback = verify_completion_contract(
            dir.path(),
            Some(&events),
            &contract,
            "create the app page",
            &mut verify_attempts,
            Some(&previous_signature),
            Some(previous_target),
            &[],
            &["src/app/widget.tsx".to_string()],
            &["src/app/widget.tsx".to_string()],
            true,
            NodeDependencySetupAuthority::None,
            false,
        )
        .unwrap()
        .expect("first repair should be re-anchored, not stopped");
        assert!(feedback.feedback.contains(
            "Previous edit did not address the failure. You must edit one of the following files:"
        ));

        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){return <main>ok</main>;}\n",
        )
        .unwrap();
        let outcome = verify_completion_contract(
            dir.path(),
            Some(&events),
            &contract,
            "create the app page",
            &mut verify_attempts,
            Some(&feedback.signature),
            Some(feedback.target),
            &["src/app/widget.tsx".to_string()],
            &[
                "src/app/widget.tsx".to_string(),
                "src/app/page.tsx".to_string(),
            ],
            &["src/app/page.tsx".to_string()],
            true,
            NodeDependencySetupAuthority::None,
            false,
        )
        .unwrap();
        assert!(outcome.is_none());
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains("\"repair_follow_through\":\"target_not_followed\""));
        assert!(event_text.contains("\"repair_follow_through\":\"target_matched\""));
        assert!(!event_text.contains("\"reason\":\"repair_target_not_followed\""));
    }

    #[test]
    fn uat_0702_source_scan_gameplay_repair_editing_page_is_target_matched() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["src/app/page.tsx"],"required_evidence":["challenge_or_adversary_evidence","failure_or_collision_evidence","restart_or_recoverable_state_evidence"],"verify_repair_cap":2}"#,
        )
        .unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        cfg.eval_events_path = Some(events.clone());
        let partial_page = r#""use client";
import { useState } from "react";
export default function Page(){
  const [score,setScore] = useState(0);
  return <main><button onClick={() => setScore(score + 1)}>Start</button><p>score {score}</p></main>;
}
"#;
        let repaired_page = r#""use client";
import { useState } from "react";
export default function Page(){
  const [gameOver,setGameOver] = useState(false);
  const [score,setScore] = useState(0);
  const [bullets,setBullets] = useState([{ x: 0, y: 0 }]);
  const [enemies,setEnemies] = useState([{ x: 1, y: 1 }]);
  const fire = () => {
    setBullets((items) => [...items, { x: 1, y: 1 }]);
    bullets.forEach((bullet) => {
      enemies.forEach((enemy) => {
        if (Math.abs(bullet.x - enemy.x) < 10 && Math.abs(bullet.y - enemy.y) < 10) {
          setGameOver(true);
          setScore((value) => value + 10);
        }
      });
    });
  };
  const restart = () => {
    setGameOver(false);
    setScore(0);
    setBullets([]);
    setEnemies([{ x: 1, y: 1 }]);
  };
  return <main><button onClick={fire}>Start</button><button onClick={restart}>Restart</button><button onClick={fire}>Fire</button><p>enemy collision score {score} {gameOver ? "game over" : "playing"}</p></main>;
}
"#;
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"src/app/page.tsx","content":partial_page}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"src/app/page.tsx","content":repaired_page}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create a game page",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap();
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::CompletionContractSatisfied
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"repair_target\":\"implementation\""));
        assert!(event_text.contains("\"repair_follow_through\":\"target_matched\""));
        assert!(!event_text.contains("repair_target_not_followed"));
    }

    #[test]
    fn run_session_string_wrapper_preserves_existing_cli_behavior() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply::text("plain final"))],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session(
            &mut fake,
            &mut session,
            "Summarize workspace",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "plain final");
    }

    #[test]
    fn changed_paths_are_workspace_relative_after_tool_success() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"src/app/page.tsx","content":"export default function Page(){return null;}"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create page",
            &["src/app/page.tsx".to_string()],
            &config(dir.path().to_path_buf()),
            &NOOP_UI,
        )
        .unwrap();
        assert_eq!(outcome.changed_paths, vec!["src/app/page.tsx"]);
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
    }

    #[test]
    fn missing_tool_argument_feedback_allows_retry() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Grep", json!({}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.txt","content":"ok"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session_with_required_paths(
            &mut fake,
            &mut session,
            "create a.txt",
            &["a.txt".to_string()],
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "required artifacts satisfied: a.txt");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "ok"
        );
        assert!(
            session
                .messages
                .iter()
                .any(|message| message.content.contains("recoverable validation error"))
        );
    }

    #[test]
    fn edit_anchor_mismatch_returns_recoverable_feedback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "actual content").unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Edit",
                        json!({"path":"a.txt","old_string":"actual missing","new_string":"replacement"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("final")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session(
            &mut fake,
            &mut session,
            "Summarize workspace",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "final");
        assert!(session.messages.iter().any(|message| message.role == "tool"
            && message.content.contains("edit_anchor_not_found")
            && message.content.contains("Deterministic best-match region")
            && message.content.contains("actual content")
            && message.content.contains("Re-anchor mandate")));
    }

    #[test]
    fn repeated_edit_anchor_failures_on_same_file_prompt_full_file_write() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "actual content\n").unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Edit",
                        json!({"path":"a.txt","old_string":"missing anchor","new_string":"replacement"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Edit",
                        json!({"path":"a.txt","old_string":"other missing anchor","new_string":"replacement"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("final")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session(
            &mut fake,
            &mut session,
            "Summarize workspace",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "final");
        assert!(
            session.messages.iter().any(|message| message.role == "tool"
                && message.content.contains("anchor failure #2")
                && message.content.contains("Write tool")
                && message.content.contains("complete corrected file content")
                && message.content.contains("`a.txt`")),
            "{:?}",
            session.messages
        );
    }

    #[test]
    fn prompt_requested_artifact_feedback_then_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply::text("done")),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.txt","content":"ok"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session(
            &mut fake,
            &mut session,
            "Create the file.\n\nRequired final artifacts:\n- a.txt",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "required artifacts satisfied: a.txt");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "ok"
        );
        assert!(
            !session
                .messages
                .iter()
                .any(|message| message.role == "assistant" && message.content == "done")
        );
    }

    #[test]
    fn completion_without_write_feedback_then_write_then_complete() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply::text("done")),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.txt","content":"ok"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session(
            &mut fake,
            &mut session,
            "create a.txt",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "done");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "ok"
        );
    }

    #[test]
    fn empty_response_gets_one_retry_feedback() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply::text("")),
                Ok(AssistantReply::text("final")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session(
            &mut fake,
            &mut session,
            "Summarize this workspace.",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "final");
        assert!(
            !session
                .messages
                .iter()
                .any(|message| message.role == "assistant" && message.content.is_empty())
        );
    }

    #[test]
    fn repeated_planned_action_without_tool_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply::text("I will create it.")),
                Ok(AssistantReply::text("I will create it now.")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let err = run_session(
            &mut fake,
            &mut session,
            "create a.txt",
            &config(dir.path().to_path_buf()),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("missing tool call for action prompt"));
    }

    #[test]
    fn provider_http_error_does_not_enable_xml_fallback() {
        assert!(!provider_error_allows_xml_fallback(&anyhow::anyhow!(
            "OpenAI Responses API failed: 500 Internal Server Error"
        )));
        assert!(provider_error_allows_xml_fallback(&anyhow::anyhow!(
            "OpenAI function_call arguments are not valid JSON"
        )));
    }

    struct ParseRetryFake {
        replies: Vec<anyhow::Result<AssistantReply>>,
        native_flags: Vec<bool>,
    }

    impl ChatClient for ParseRetryFake {
        fn label(&self) -> &str {
            "parse-retry-fake"
        }

        fn supports_native_tools(&self, _model: &str) -> bool {
            true
        }

        fn allows_xml_fallback(&self) -> bool {
            true
        }

        fn chat(
            &mut self,
            _model: &str,
            _messages: &[ConversationMessage],
            _tools: &[crate::tools::registry::ToolSpec],
            native_tools_enabled: bool,
        ) -> anyhow::Result<AssistantReply> {
            self.native_flags.push(native_tools_enabled);
            self.replies.remove(0)
        }
    }

    #[test]
    fn malformed_native_tool_call_retries_with_native_tools_before_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = ParseRetryFake {
            replies: vec![
                Err(anyhow::anyhow!(
                    "OpenAI function_call arguments are not valid JSON"
                )),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.txt","content":"ok"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
            native_flags: Vec::new(),
        };
        let mut session = SessionSnapshot::new();
        let result = run_session_with_required_paths(
            &mut fake,
            &mut session,
            "create a.txt",
            &["a.txt".to_string()],
            &config(dir.path().to_path_buf()),
        )
        .unwrap();

        assert_eq!(result, "required artifacts satisfied: a.txt");
        assert_eq!(fake.native_flags, vec![true, true]);
        assert!(!session.native_tools_disabled);
    }

    #[test]
    fn repeated_malformed_native_tool_call_eventually_uses_xml_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = ParseRetryFake {
            replies: vec![
                Err(anyhow::anyhow!(
                    "OpenAI function_call arguments are not valid JSON"
                )),
                Err(anyhow::anyhow!(
                    "OpenAI function_call arguments are not valid JSON"
                )),
                Err(anyhow::anyhow!(
                    "OpenAI function_call arguments are not valid JSON"
                )),
                Ok(AssistantReply::text("final")),
            ],
            native_flags: Vec::new(),
        };
        let mut session = SessionSnapshot::new();
        let result = run_session(
            &mut fake,
            &mut session,
            "Summarize this workspace.",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();

        assert_eq!(result, "final");
        assert_eq!(fake.native_flags, vec![true, true, true, false]);
        assert!(session.native_tools_disabled);
    }

    #[test]
    fn requested_artifact_path_extraction_rejects_escape_and_metadata_paths() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = "\
Required final artifacts:
- ../outside.txt
- /tmp/out.txt
- .anvil/session.json
- target/debug/app
- node_modules/pkg/index.js
- package.json
- src/app/page.tsx
";
        let paths = extract_requested_artifact_paths(dir.path(), prompt);
        assert_eq!(paths, vec!["package.json", "src/app/page.tsx"]);
    }

    #[test]
    fn requested_artifact_path_extraction_rejects_backticked_escape() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = "Create `src/main.rs`, not `../main.rs` or `.anvil/log.json`.";
        let paths = extract_requested_artifact_paths(dir.path(), prompt);
        assert_eq!(paths, vec!["src/main.rs"]);
    }

    #[test]
    #[cfg(unix)]
    fn requested_artifact_path_extraction_rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/tmp", dir.path().join("out")).unwrap();
        let prompt = "\
Required final artifacts:
- out/file.txt
- safe/file.txt
";
        let paths = extract_requested_artifact_paths(dir.path(), prompt);
        assert_eq!(paths, vec!["safe/file.txt"]);
    }

    #[test]
    fn missing_relative_import_gets_repair_prompt_before_final() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"src/page.tsx","content":"import Widget from './Widget';\nexport default function Page(){return <Widget/>;}"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"src/Widget.tsx","content":"export default function Widget(){return <div/>;}"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session(
            &mut fake,
            &mut session,
            "create a small Next.js page",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "done");
        assert!(dir.path().join("src/Widget.tsx").is_file());
        assert_eq!(
            session
                .messages
                .iter()
                .filter(|message| message.role == "assistant" && message.content == "done")
                .count(),
            1
        );
    }

    #[test]
    fn required_artifact_success_waits_for_missing_relative_imports() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"src/app/page.tsx","content":"import './layout.css';\nexport default function Page(){return <main/>;}"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"src/app/layout.css","content":"main { color: white; }\n"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create a page",
            &["src/app/page.tsx".to_string()],
            &config(dir.path().to_path_buf()),
            &NOOP_UI,
        )
        .unwrap();
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert!(dir.path().join("src/app/layout.css").is_file());
        assert_eq!(outcome.changed_paths.len(), 2);
    }

    #[test]
    fn repeated_recoverable_tool_error_stops_before_max_iterations() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "ok").unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.max_iterations = 8;
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Read", json!({"path":"workdir/a.txt"}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Read", json!({"path":"workdir/a.txt"}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Read", json!({"path":"workdir/a.txt"}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let err = run_session(&mut fake, &mut session, "read a.txt", &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("recoverable tool error repeated"));
    }

    #[test]
    fn repeated_verify_policy_error_substitutes_expected_path_check() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events_path.clone());
        cfg.max_iterations = 5;
        let rejected = "npm run build | grep error";
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Bash", json!({"command": rejected}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Bash", json!({"command": rejected}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Verify the current step.",
            &["package.json".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Verify),
        )
        .unwrap();

        assert_eq!(outcome.final_text, "done");
        let bash_result = session
            .messages
            .iter()
            .rfind(|message| message.role == "tool" && message.name.as_deref() == Some("Bash"))
            .map(|message| message.content.as_str())
            .unwrap_or_default();
        assert!(bash_result.contains("outcome: Success"), "{bash_result}");
        let events = event_values(&events_path);
        let substitution = events
            .iter()
            .find(|event| {
                event.get("event").and_then(Value::as_str) == Some("verify_command_substituted")
            })
            .expect("substitution event");
        assert_eq!(
            substitution.get("reason").and_then(Value::as_str),
            Some("policy_repetition")
        );
        assert_eq!(
            substitution.get("oracle_tier").and_then(Value::as_str),
            Some("degraded")
        );
        assert_eq!(
            substitution.get("substitute").and_then(Value::as_str),
            Some("test -f package.json")
        );
        assert!(
            events.iter().any(|event| {
                event.get("event").and_then(Value::as_str) == Some("step_oracle_tier_degraded")
            }),
            "{events:?}"
        );
        assert!(
            !events.iter().any(|event| {
                event.get("event").and_then(Value::as_str) == Some("loop_stop")
                    && event.get("reason").and_then(Value::as_str)
                        == Some("recoverable_tool_error_repeated")
            }),
            "{events:?}"
        );
    }

    #[test]
    fn unrelated_edits_do_not_reset_artifact_recovery_targeting() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["a.txt"],"verify_commands":[]}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        cfg.max_iterations = 6;
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"scratch1.txt","content":"x"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"scratch2.txt","content":"x"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"scratch3.txt","content":"x"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.txt","content":"ok"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create a.txt",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap();
        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
    }

    #[test]
    fn artifact_recovery_event_records_target_path() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        let events = dir.path().join("events.jsonl");
        std::fs::write(
            &contract,
            r#"{"required_paths":["a.txt","b.txt"],"verify_commands":[]}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 13;
        let mut replies = Vec::new();
        for _ in 0..12 {
            replies.push(Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new("Glob", json!({"pattern":"**/*"}))],
                prompt_tokens: None,
                completion_tokens: None,
            }));
        }
        let mut fake = Fake { replies };
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_ui(
            &mut fake,
            &mut session,
            "create required files",
            &[],
            &cfg,
            &NOOP_UI,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("artifact recovery exhausted"));
        let text = std::fs::read_to_string(events).unwrap();
        assert!(text.contains("\"target_path\":\"a.txt\""));
        assert!(text.contains("\"last_target_path\":\"a.txt\""));
    }

    #[test]
    fn dangerous_command_remains_hard_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new("Bash", json!({"command":"rm -rf /"}))],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let err = run_session(
            &mut fake,
            &mut session,
            "run command",
            &config(dir.path().to_path_buf()),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("dangerous command blocked"));
    }
}
