use std::path::{Path, PathBuf};

use super::artifact_completion_job::{AllowedReadScope, AllowedWriteActions};
use super::task_workspace_scope::TaskWorkspaceScope;
use crate::ollama::xml_fallback::ToolCall;
use crate::safety::path_guard::resolve_user_path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EffectiveToolPolicy {
    pub(super) allowed_tools: Option<Vec<&'static str>>,
    pub(super) focused_edit: Option<FocusedEditPolicy>,
    pub(super) artifact_directed: Option<ArtifactDirectedPolicy>,
    pub(super) reason: EffectiveToolPolicyReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FocusedEditPolicy {
    pub(super) target: PathBuf,
    pub(super) target_already_read: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ArtifactDirectedPolicy {
    pub(super) target: PathBuf,
    pub(super) target_already_read: bool,
}

/// Issue #664 (AD1 / AD6 / DC1-002): `#[non_exhaustive]` enables additive
/// variant extensions (e.g. `SetupBootstrap`) without breaking external
/// consumers' exhaustive match. In-crate consumers still get compile errors
/// on `match` arms when the closed list grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(super) enum EffectiveToolPolicyReason {
    Unrestricted,
    AnswerOnly,
    VerifierRepair,
    ArtifactDirectedRecovery,
    FocusedEditRecovery,
    LocalLlmSmallEditAfterRead,
    /// Issue #664: SetupBootstrap policy projection — `Bash` only,
    /// command-level filter via `crate::tools::bash::is_setup_command`.
    SetupBootstrap,
    /// Objective evidence collection policy. This is not a task-kind mode:
    /// the ObjectiveContract has declared that the next missing authority is
    /// an observed local command result.
    EvidenceAction,
}

impl EffectiveToolPolicyReason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Unrestricted => "unrestricted",
            Self::AnswerOnly => "answer_only",
            Self::VerifierRepair => "verifier_repair",
            Self::ArtifactDirectedRecovery => "artifact_directed_recovery",
            Self::FocusedEditRecovery => "focused_edit_recovery",
            Self::LocalLlmSmallEditAfterRead => "local_llm_small_edit_after_read",
            Self::SetupBootstrap => "setup_bootstrap",
            Self::EvidenceAction => "evidence_action",
        }
    }
}

impl EffectiveToolPolicy {
    pub(super) fn unrestricted() -> Self {
        Self {
            allowed_tools: None,
            focused_edit: None,
            artifact_directed: None,
            reason: EffectiveToolPolicyReason::Unrestricted,
        }
    }

    pub(super) fn restricted(
        reason: EffectiveToolPolicyReason,
        allowed_tools: Vec<&'static str>,
    ) -> Self {
        Self {
            allowed_tools: Some(allowed_tools),
            focused_edit: None,
            artifact_directed: None,
            reason,
        }
    }

    /// Issue #663 (CB-001 fix): test-only legacy constructor. Production
    /// builds drive ArtifactRecovery exclusively via
    /// `artifact_directed_from_job` so that `AllowedWriteActions` /
    /// `AllowedReadScope` always originate from a validated
    /// `ArtifactCompletionJob`. This constructor is retained behind
    /// `cfg(test)` to keep regression tests for the legacy filesystem-
    /// derived projection compilable.
    #[cfg(test)]
    pub(super) fn artifact_directed(target: PathBuf, target_already_read: bool) -> Self {
        let allowed_tools = if !target.is_file() {
            vec!["Write"]
        } else if target_already_read {
            vec!["Write", "Edit"]
        } else {
            vec!["Read", "Write", "Edit"]
        };
        Self {
            allowed_tools: Some(allowed_tools),
            focused_edit: None,
            artifact_directed: Some(ArtifactDirectedPolicy {
                target,
                target_already_read,
            }),
            reason: EffectiveToolPolicyReason::ArtifactDirectedRecovery,
        }
    }

    /// Job-projection constructor that derives the allowed-tools set from
    /// the `ArtifactCompletionJob`'s explicit `AllowedWriteActions` +
    /// `AllowedReadScope`, not from `target.is_file()` alone.
    pub(super) fn artifact_directed_from_job(
        target: PathBuf,
        target_already_read: bool,
        allowed_write_actions: &AllowedWriteActions,
        allowed_read_scope: &AllowedReadScope,
    ) -> Self {
        let mut allowed_tools: Vec<&'static str> = Vec::new();
        let read_scope_allows_target_read = matches!(
            allowed_read_scope,
            AllowedReadScope::TargetOnly | AllowedReadScope::TargetAndDeps(_)
        );
        if read_scope_allows_target_read && !target_already_read {
            allowed_tools.push("Read");
        }
        if allowed_write_actions.allow_create() || allowed_write_actions.allow_modify() {
            allowed_tools.push("Write");
        }
        if allowed_write_actions.allow_modify() {
            allowed_tools.push("Edit");
        }
        Self {
            allowed_tools: Some(allowed_tools),
            focused_edit: None,
            artifact_directed: Some(ArtifactDirectedPolicy {
                target,
                target_already_read,
            }),
            reason: EffectiveToolPolicyReason::ArtifactDirectedRecovery,
        }
    }

    pub(super) fn focused_edit(
        reason: EffectiveToolPolicyReason,
        allowed_tools: Vec<&'static str>,
        target: PathBuf,
        target_already_read: bool,
    ) -> Self {
        Self {
            allowed_tools: Some(allowed_tools),
            focused_edit: Some(FocusedEditPolicy {
                target,
                target_already_read,
            }),
            artifact_directed: None,
            reason,
        }
    }

    /// Issue #664 (AD1 / 判断 6 / DR2-002 ISP): SetupBootstrap policy
    /// builder. `allowed_tools = Some(vec!["Bash"])` only — no Read /
    /// Write / Edit. The command-level filter
    /// (`crate::tools::bash::is_setup_command`) is applied at tool
    /// enforcement time in
    /// `effective_tool_policy_error_for_call_with_scope`.
    pub(super) fn setup_bootstrap() -> Self {
        Self {
            allowed_tools: Some(vec!["Bash"]),
            focused_edit: None,
            artifact_directed: None,
            reason: EffectiveToolPolicyReason::SetupBootstrap,
        }
    }

    pub(super) fn evidence_action_bash_only() -> Self {
        Self {
            allowed_tools: Some(vec!["Bash"]),
            focused_edit: None,
            artifact_directed: None,
            reason: EffectiveToolPolicyReason::EvidenceAction,
        }
    }

    pub(super) fn evidence_action_artifact_binding(target: PathBuf) -> Self {
        Self {
            allowed_tools: Some(vec!["Read", "Write", "Edit"]),
            focused_edit: None,
            artifact_directed: Some(ArtifactDirectedPolicy {
                target,
                target_already_read: false,
            }),
            reason: EffectiveToolPolicyReason::EvidenceAction,
        }
    }

    pub(super) fn allowed_tool_names_for_prompt(&self) -> Option<&[&str]> {
        self.allowed_tools.as_deref()
    }

    pub(super) fn focused_edit_policy(&self) -> Option<&FocusedEditPolicy> {
        self.focused_edit.as_ref()
    }

    pub(super) fn artifact_directed_policy(&self) -> Option<&ArtifactDirectedPolicy> {
        self.artifact_directed.as_ref()
    }

    pub(super) fn reason(&self) -> EffectiveToolPolicyReason {
        self.reason
    }
}

pub(super) fn focused_edit_policy_violation_feedback_note(
    unresolved_errors: &[String],
    allowed_tools: Option<&[&str]>,
    target_display: Option<&str>,
) -> Option<String> {
    let error = unresolved_errors.iter().rev().find(|error| {
        let is_policy_error = error.starts_with("focused edit recovery rejected ")
            || error.starts_with("focused edit recovery only allows ")
            || error.starts_with("artifact-directed recovery rejected ")
            || error.starts_with("tool policy rejected ");
        is_policy_error && target_display.is_none_or(|target| error.contains(target))
    })?;
    let allowed = allowed_tools
        .filter(|tools| !tools.is_empty())
        .map(|tools| tools.join(", "))
        .unwrap_or_else(|| "none".to_string());
    Some(format!(
        "[Focused Edit Policy Violation] Previous tool call was rejected and was not executed: {error}. Allowed tools now: {allowed}. Emit exactly one allowed tool call on the required target path; do not call omitted tools."
    ))
}

pub(super) fn focused_edit_tool_policy_error(
    name: &str,
    arguments: &serde_json::Value,
    target: &Path,
    work_root: &Path,
    target_already_read: bool,
) -> Option<String> {
    // Issue #931 (Choke C): this error string is retained in `unresolved_errors`
    // and rendered verbatim into a model-facing system note by
    // `focused_edit_policy_violation_feedback_note`, a path that does NOT pass
    // through `mask_payload_inplace`. Mask the path token at the render point so a
    // secret-shaped path cannot leak; `mask_secrets` is a no-op on ordinary paths
    // so the instruction sentence stays byte-identical for actionable targets.
    let path_display = super::task_contract::mask_and_cap_recovery_field(
        &target
            .strip_prefix(work_root)
            .unwrap_or(target)
            .to_string_lossy()
            .replace('\\', "/"),
    );
    let path_matches = arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|raw_path| {
            super::tool_history::tool_path_matches_target(raw_path, target, work_root)
        });
    let rejected_tool = compact_tool_name_for_policy_feedback(name);

    if !target.is_file() {
        if name != "Write" || !path_matches {
            return Some(format!(
                "focused edit recovery rejected {rejected_tool}; only allows Write on missing target {path_display}"
            ));
        }
        return None;
    }

    if target_already_read {
        if name != "Edit" || !path_matches {
            return Some(format!(
                "focused edit recovery rejected {rejected_tool}; only allows Edit on {path_display} after the file has already been read"
            ));
        }
        return None;
    }

    match name {
        "Read" | "Edit" if path_matches => None,
        _ => Some(format!(
            "focused edit recovery rejected {rejected_tool}; only allows Read or Edit on {path_display} until the first edit succeeds"
        )),
    }
}

/// Legacy non-scope wrapper kept for unit tests and tests-only re-exports.
/// Production code MUST use [`effective_tool_policy_error_for_call_with_scope`]
/// so the MissingVerifierJob scope gate fires (Issue #646 A1/A3).
#[cfg(test)]
pub(super) fn effective_tool_policy_error_for_call(
    policy: &EffectiveToolPolicy,
    name: &str,
    arguments: &serde_json::Value,
    work_root: &Path,
) -> Option<String> {
    effective_tool_policy_error_for_call_with_scope(policy, name, arguments, work_root, None)
}

/// Issue #646 (A1/A3): scope-aware variant. When `scope` is provided and the
/// policy is a `VerifierRepair`-reasoned restricted policy WITHOUT a
/// focused-edit or artifact-directed target (i.e. the MissingVerifierJob
/// fallback whitelist), the file-targeting tool calls must additionally pass
/// `scope.contains(...)` on the resolved path argument. Out-of-scope writes
/// are rejected even though `Write`/`Edit`/`Bash` would otherwise satisfy
/// the tool-name whitelist.
pub(super) fn effective_tool_policy_error_for_call_with_scope(
    policy: &EffectiveToolPolicy,
    name: &str,
    arguments: &serde_json::Value,
    work_root: &Path,
    scope: Option<&TaskWorkspaceScope>,
) -> Option<String> {
    if let Some(allowed_tools) = policy.allowed_tool_names_for_prompt()
        && !allowed_tools.contains(&name)
    {
        // Issue #664 iteration-3 (CB2-003): when the LLM calls `Bash`
        // under an artifact-directed recovery policy (Read/Write/Edit-
        // only allow set), the allow-list check fires *before* the
        // SetupBootstrap branch below could classify the rejection as a
        // BashOutOfPolicy. To keep the caller-side string-match in
        // `execute_tool_call_with_optional_policy_resolution` reachable
        // (which routes the rejection through
        // `record_artifact_completion_bash_violation` so the structured
        // report emits `category = "bash_out_of_policy"`), we emit the
        // same `"artifact-directed recovery rejected Bash"` shape that
        // the artifact-directed path uses. Otherwise the rejection
        // would surface as the generic `"tool policy rejected Bash"`
        // string and the BashOutOfPolicy branch would be unreachable.
        if policy.reason() == EffectiveToolPolicyReason::ArtifactDirectedRecovery && name == "Bash"
        {
            // Issue #931 (Choke C, CB-001): this rejection string is retained in
            // `unresolved_errors` and surfaced to the model as a tool result, so
            // mask the path token at the render point (symmetric with the other
            // policy-error producers and the masked filter target so the follow-up
            // policy-violation note still fires). No-op on ordinary paths.
            let target_display = policy_target_path(policy)
                .map(|target| {
                    super::task_contract::mask_and_cap_recovery_field(
                        &target
                            .strip_prefix(work_root)
                            .unwrap_or(target)
                            .to_string_lossy()
                            .replace('\\', "/"),
                    )
                })
                .unwrap_or_else(|| "the active target".to_string());
            return Some(format!(
                "artifact-directed recovery rejected Bash; only allows Write or Edit on {target_display}; Read may inspect workspace files"
            ));
        }
        return Some(restricted_tool_policy_error(
            policy,
            name,
            allowed_tools,
            work_root,
        ));
    }

    // Issue #664 (DS1-001 二段防衛): SetupBootstrap allows `Bash` only at
    // the tool-name layer, but `cargo test` / `echo hi` / `curl ...` would
    // pass the allowlist. The command-level allow set is decided here via
    // `crate::tools::bash::is_setup_command`. Non-Setup commands are
    // rejected with a clear error.
    if policy.reason() == EffectiveToolPolicyReason::SetupBootstrap && name == "Bash" {
        let Some(command) = arguments.get("command").and_then(serde_json::Value::as_str) else {
            return Some("setup bootstrap bash requires a string `command` argument".to_string());
        };
        if !crate::tools::bash::is_setup_command(command) {
            return Some(
                "setup bootstrap only allows dependency-install Bash commands (BashCommandClass::EnvSetup)"
                    .to_string(),
            );
        }
    }

    if let Some(focused) = policy.focused_edit_policy() {
        return focused_edit_tool_policy_error(
            name,
            arguments,
            &focused.target,
            work_root,
            focused.target_already_read,
        );
    }

    if let Some(artifact) = policy.artifact_directed_policy() {
        return artifact_directed_tool_policy_error(name, arguments, &artifact.target, work_root);
    }

    // Issue #646 (A1/A3): MissingVerifierJob fallback scope enforcement.
    if policy.reason() == EffectiveToolPolicyReason::VerifierRepair
        && let Some(scope) = scope
        && let Some(err) = missing_verifier_scope_policy_error(name, arguments, work_root, scope)
    {
        return Some(err);
    }

    None
}

/// Issue #646 (A1/A3): rejects file-targeting tool calls whose resolved path
/// argument falls outside the active `TaskWorkspaceScope`. Applies only when
/// the policy reason is `VerifierRepair` and the policy carries no specific
/// target (i.e. the MissingVerifierJob whitelist case).
fn missing_verifier_scope_policy_error(
    name: &str,
    arguments: &serde_json::Value,
    work_root: &Path,
    scope: &TaskWorkspaceScope,
) -> Option<String> {
    if !matches!(name, "Write" | "Edit") {
        return None;
    }
    let raw_path = arguments.get("path").and_then(serde_json::Value::as_str)?;
    let relative = workspace_relative_path_for_tool_arg(work_root, raw_path)?;
    if scope.contains(&relative) {
        return None;
    }
    let rejected_tool = compact_tool_name_for_policy_feedback(name);
    Some(format!(
        "MissingVerifierJob policy rejected {rejected_tool}; path {relative} is outside the active workspace scope"
    ))
}

pub(super) fn artifact_directed_tool_policy_error(
    name: &str,
    arguments: &serde_json::Value,
    target: &Path,
    work_root: &Path,
) -> Option<String> {
    if name == "Read"
        && arguments
            .get("path")
            .and_then(serde_json::Value::as_str)
            .and_then(|raw_path| workspace_relative_path_for_tool_arg(work_root, raw_path))
            .is_some()
    {
        return None;
    }
    // CB-003: SSOT target match — compare on the workspace-relative
    // canonical form derived from `resolve_user_path` (used by both
    // `task_workspace_scope` and `ArtifactCompletionJob.target_path`).
    let path_matches = arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|raw_path| {
            tool_path_matches_target_via_workspace_ssot(raw_path, target, work_root)
        });
    if path_matches {
        return None;
    }

    let rejected_tool = compact_tool_name_for_policy_feedback(name);
    // Issue #931 (Choke C): mask the path token at the render point — this string
    // reaches a model-facing system note via `unresolved_errors` →
    // `focused_edit_policy_violation_feedback_note`. No-op on ordinary paths.
    let path_display = super::task_contract::mask_and_cap_recovery_field(
        &target
            .strip_prefix(work_root)
            .unwrap_or(target)
            .to_string_lossy()
            .replace('\\', "/"),
    );
    Some(format!(
        "artifact-directed recovery rejected {rejected_tool}; only allows Write or Edit on {path_display}; Read may inspect workspace files"
    ))
}

/// CB-003 / CB2-002: SSOT target match for the artifact-directed policy gate.
pub(super) fn tool_path_matches_target_via_workspace_ssot(
    raw_path: &str,
    target: &Path,
    work_root: &Path,
) -> bool {
    let Some(input_rel) = workspace_relative_path_for_tool_arg(work_root, raw_path) else {
        return false;
    };
    let target_str = target.to_string_lossy();
    let Some(target_rel) = workspace_relative_path_for_tool_arg(work_root, &target_str) else {
        return false;
    };
    if input_rel != target_rel {
        return false;
    }
    if !ancestor_chain_has_no_dangling_symlinks(work_root, raw_path) {
        return false;
    }
    if !ancestor_chain_has_no_dangling_symlinks(work_root, &target_str) {
        return false;
    }
    true
}

fn ancestor_chain_has_no_dangling_symlinks(work_root: &Path, raw_path: &str) -> bool {
    let input = Path::new(raw_path);
    let candidate: PathBuf = if input.is_absolute() {
        input.to_path_buf()
    } else {
        work_root.join(input)
    };
    super::artifact_ownership::nearest_existing_ancestor_within_work_root(work_root, &candidate)
}

fn restricted_tool_policy_error(
    policy: &EffectiveToolPolicy,
    name: &str,
    allowed_tools: &[&str],
    work_root: &Path,
) -> String {
    let rejected_tool = compact_tool_name_for_policy_feedback(name);
    let allowed = if allowed_tools.is_empty() {
        "none".to_string()
    } else {
        allowed_tools.join(", ")
    };
    let mut message = format!(
        "tool policy rejected {rejected_tool}; allowed tools: {allowed}; reason: {}",
        policy.reason().as_str()
    );
    if let Some(target) = policy_target_path(policy) {
        // Issue #931 (Choke C): mask the path token at the render point — this
        // string can reach a model-facing system note via `unresolved_errors`.
        // No-op on ordinary paths, so byte-identical for actionable targets.
        let path_display = super::task_contract::mask_and_cap_recovery_field(
            &target
                .strip_prefix(work_root)
                .unwrap_or(target)
                .to_string_lossy()
                .replace('\\', "/"),
        );
        message.push_str("; target: ");
        message.push_str(&path_display);
    }
    message
}

fn policy_target_path(policy: &EffectiveToolPolicy) -> Option<&Path> {
    policy
        .focused_edit_policy()
        .map(|focused| focused.target.as_path())
        .or_else(|| {
            policy
                .artifact_directed_policy()
                .map(|artifact| artifact.target.as_path())
        })
}

fn compact_tool_name_for_policy_feedback(name: &str) -> String {
    let mut compact = name
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        .take(32)
        .collect::<String>();
    if compact.is_empty() {
        compact.push_str("unknown-tool");
    }
    compact
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FocusedEditBatchAction {
    Accept,
    TruncateToFirst,
    Reject(String),
}

pub(super) fn focused_edit_tool_batch_action(
    tool_calls: &[ToolCall],
    target: &Path,
    work_root: &Path,
    target_already_read: bool,
) -> FocusedEditBatchAction {
    let Some(first_tool_call) = tool_calls.first() else {
        return FocusedEditBatchAction::Accept;
    };

    let first_call_error = focused_edit_tool_policy_error(
        &first_tool_call.name,
        &first_tool_call.arguments,
        target,
        work_root,
        target_already_read,
    );

    if tool_calls.len() == 1 {
        return first_call_error
            .map(FocusedEditBatchAction::Reject)
            .unwrap_or(FocusedEditBatchAction::Accept);
    }

    if first_call_error.is_none() {
        FocusedEditBatchAction::TruncateToFirst
    } else {
        FocusedEditBatchAction::Reject(first_call_error.unwrap_or_default())
    }
}

/// Legacy non-scope wrapper kept for unit tests and tests-only re-exports.
/// Production code MUST use [`effective_tool_batch_action_with_scope`].
#[cfg(test)]
pub(super) fn effective_tool_batch_action(
    tool_calls: &[ToolCall],
    policy: &EffectiveToolPolicy,
    work_root: &Path,
) -> FocusedEditBatchAction {
    effective_tool_batch_action_with_scope(tool_calls, policy, work_root, None)
}

/// Issue #646 (A1/A3): scope-aware variant of [`effective_tool_batch_action`].
/// Forwarded scope is consulted only by the MissingVerifierJob fallback
/// branch inside `effective_tool_policy_error_for_call_with_scope`.
pub(super) fn effective_tool_batch_action_with_scope(
    tool_calls: &[ToolCall],
    policy: &EffectiveToolPolicy,
    work_root: &Path,
    scope: Option<&TaskWorkspaceScope>,
) -> FocusedEditBatchAction {
    let Some(first_tool_call) = tool_calls.first() else {
        return FocusedEditBatchAction::Accept;
    };

    if let Some(err) = effective_tool_policy_error_for_call_with_scope(
        policy,
        &first_tool_call.name,
        &first_tool_call.arguments,
        work_root,
        scope,
    ) {
        return FocusedEditBatchAction::Reject(err);
    }

    if let Some(focused) = policy.focused_edit_policy() {
        return focused_edit_tool_batch_action(
            tool_calls,
            &focused.target,
            work_root,
            focused.target_already_read,
        );
    }

    if policy.artifact_directed_policy().is_some() && tool_calls.len() > 1 {
        return FocusedEditBatchAction::TruncateToFirst;
    }

    FocusedEditBatchAction::Accept
}

pub(super) fn workspace_relative_path_for_tool_arg(
    work_root: &Path,
    raw_path: &str,
) -> Option<String> {
    let resolved = resolve_user_path(work_root, raw_path).ok()?;
    let root = std::fs::canonicalize(work_root).unwrap_or_else(|_| work_root.to_path_buf());
    let relative = resolved.strip_prefix(root).ok()?;
    Some(relative.to_string_lossy().replace('\\', "/"))
}
