use std::path::PathBuf;

use super::artifact_completion_job::{AllowedReadScope, AllowedWriteActions};

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
