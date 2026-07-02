use serde::Serialize;

use crate::minimal_loop::build_verifier::{BuildVerifierLifecycleObservation, BuildVerifierStatus};
use crate::minimal_loop::dependency_setup::NodeDependencySetupStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifierBootstrapState {
    NoVerifierRequired,
    VerifierRequired,
    VerifierMissing,
    DependencySetupRequired,
    DependencySetupBlocked,
    DependencySetupFailed,
    VerifierReady,
    VerifierPassed,
    VerifierFailed,
}

impl VerifierBootstrapState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoVerifierRequired => "no_verifier_required",
            Self::VerifierRequired => "verifier_required",
            Self::VerifierMissing => "verifier_missing",
            Self::DependencySetupRequired => "dependency_setup_required",
            Self::DependencySetupBlocked => "dependency_setup_blocked",
            Self::DependencySetupFailed => "dependency_setup_failed",
            Self::VerifierReady => "verifier_ready",
            Self::VerifierPassed => "verifier_passed",
            Self::VerifierFailed => "verifier_failed",
        }
    }
}

pub fn state_from_lifecycles(
    build_verifier_required: bool,
    lifecycles: &[BuildVerifierLifecycleObservation],
) -> VerifierBootstrapState {
    if !build_verifier_required {
        return VerifierBootstrapState::NoVerifierRequired;
    }
    if lifecycles.is_empty() {
        return VerifierBootstrapState::VerifierMissing;
    }
    if lifecycles
        .iter()
        .all(|lifecycle| lifecycle.final_status == BuildVerifierStatus::Passed)
    {
        return VerifierBootstrapState::VerifierPassed;
    }
    if lifecycles
        .iter()
        .any(|lifecycle| lifecycle.final_status == BuildVerifierStatus::Failed)
    {
        return VerifierBootstrapState::VerifierFailed;
    }
    if lifecycles.iter().any(|lifecycle| {
        lifecycle
            .setup
            .as_ref()
            .is_some_and(|setup| setup.status == NodeDependencySetupStatus::Failed)
    }) {
        return VerifierBootstrapState::DependencySetupFailed;
    }
    if lifecycles.iter().any(|lifecycle| {
        lifecycle.setup.as_ref().is_some_and(|setup| {
            matches!(
                setup.status,
                NodeDependencySetupStatus::Blocked | NodeDependencySetupStatus::TimedOut
            )
        })
    }) {
        return VerifierBootstrapState::DependencySetupBlocked;
    }
    if lifecycles
        .iter()
        .any(|lifecycle| lifecycle.final_status == BuildVerifierStatus::DependencyMissing)
    {
        return VerifierBootstrapState::DependencySetupRequired;
    }
    if lifecycles.iter().any(|lifecycle| {
        lifecycle.before_setup.attempted
            || lifecycle
                .after_setup
                .as_ref()
                .is_some_and(|observation| observation.attempted)
    }) {
        return VerifierBootstrapState::VerifierReady;
    }
    VerifierBootstrapState::VerifierRequired
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minimal_loop::build_verifier::{
        BuildVerifierLifecycleObservation, BuildVerifierObservation, BuildVerifierRequirement,
    };

    fn lifecycle(
        status: BuildVerifierStatus,
        attempted: bool,
    ) -> BuildVerifierLifecycleObservation {
        let requirement = BuildVerifierRequirement {
            command: "npm run build".to_string(),
            profile: Some("nextjs".to_string()),
            reason: "test".to_string(),
            authority: "test".to_string(),
            status: "required".to_string(),
            requires_dependency_setup: true,
            required_for_completion: true,
        };
        let before_setup = BuildVerifierObservation {
            command: requirement.command.clone(),
            profile: requirement.profile.clone(),
            authority: requirement.authority.clone(),
            required_for_completion: true,
            requires_dependency_setup: true,
            dependency_ready: status != BuildVerifierStatus::DependencyMissing,
            attempted,
            status,
            primary_reason: "test".to_string(),
            output_snippet: String::new(),
            foreign_toolchain: None,
        };
        BuildVerifierLifecycleObservation {
            requirement,
            before_setup,
            setup: None,
            after_setup: None,
            final_status: status,
            final_reason: "test".to_string(),
        }
    }

    #[test]
    fn no_required_verifier_is_not_required() {
        assert_eq!(
            state_from_lifecycles(false, &[]),
            VerifierBootstrapState::NoVerifierRequired
        );
    }

    #[test]
    fn dependency_missing_maps_to_setup_required() {
        assert_eq!(
            state_from_lifecycles(
                true,
                &[lifecycle(BuildVerifierStatus::DependencyMissing, false)]
            ),
            VerifierBootstrapState::DependencySetupRequired
        );
    }

    #[test]
    fn passed_maps_to_verifier_passed() {
        assert_eq!(
            state_from_lifecycles(true, &[lifecycle(BuildVerifierStatus::Passed, true)]),
            VerifierBootstrapState::VerifierPassed
        );
    }
}
