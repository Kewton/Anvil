//! Authoring-style decisions are separate from verifier-runner admission.
//!
//! A runner answers "how evidence is collected"; an authoring style answers
//! "what shape the model should use when creating artifacts". Keeping them
//! separate prevents an evidence command from accidentally becoming a generation
//! style requirement.

use super::verifier_command_policy::PythonProjectUnitVerifierFlavor;
use super::worker_contract::RuntimeProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AuthoringStyle {
    Unspecified,
    PytestFunctionStyle,
    UnittestClassStyle,
    ExistingProjectStyle,
}

impl AuthoringStyle {
    pub(super) fn label(self) -> &'static str {
        match self {
            AuthoringStyle::Unspecified => "unspecified",
            AuthoringStyle::PytestFunctionStyle => "pytest_function_style",
            AuthoringStyle::UnittestClassStyle => "unittest_class_style",
            AuthoringStyle::ExistingProjectStyle => "existing_project_style",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StyleAuthority {
    ExplicitUserRequest,
    ExistingProjectConvention,
    RuntimeCapability,
    ModelRobustDefault,
    Unknown,
}

impl StyleAuthority {
    pub(super) fn label(self) -> &'static str {
        match self {
            StyleAuthority::ExplicitUserRequest => "explicit_user_request",
            StyleAuthority::ExistingProjectConvention => "existing_project_convention",
            StyleAuthority::RuntimeCapability => "runtime_capability",
            StyleAuthority::ModelRobustDefault => "model_robust_default",
            StyleAuthority::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AuthoringStyleDecision {
    pub(super) style: AuthoringStyle,
    pub(super) authority: StyleAuthority,
}

impl AuthoringStyleDecision {
    pub(super) const fn new(style: AuthoringStyle, authority: StyleAuthority) -> Self {
        Self { style, authority }
    }

    pub(super) const fn unspecified() -> Self {
        Self::new(AuthoringStyle::Unspecified, StyleAuthority::Unknown)
    }

    pub(super) fn summary(self) -> String {
        format!(
            "style={},authority={}",
            self.style.label(),
            self.authority.label()
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct PythonAuthoringStyleSignals {
    pub(super) explicit_pytest_requested: bool,
    pub(super) explicit_unittest_requested: bool,
    pub(super) existing_pytest_convention: bool,
    pub(super) existing_unittest_convention: bool,
    pub(super) runtime_capability_style_required: bool,
    pub(super) model_robust_pytest_default: bool,
}

pub(super) fn decide_python_authoring_style(
    signals: PythonAuthoringStyleSignals,
    verifier_flavor: PythonProjectUnitVerifierFlavor,
) -> AuthoringStyleDecision {
    if signals.explicit_unittest_requested {
        return AuthoringStyleDecision::new(
            AuthoringStyle::UnittestClassStyle,
            StyleAuthority::ExplicitUserRequest,
        );
    }
    if signals.explicit_pytest_requested {
        return AuthoringStyleDecision::new(
            AuthoringStyle::PytestFunctionStyle,
            StyleAuthority::ExplicitUserRequest,
        );
    }
    if signals.existing_unittest_convention {
        return AuthoringStyleDecision::new(
            AuthoringStyle::ExistingProjectStyle,
            StyleAuthority::ExistingProjectConvention,
        );
    }
    if signals.existing_pytest_convention {
        return AuthoringStyleDecision::new(
            AuthoringStyle::ExistingProjectStyle,
            StyleAuthority::ExistingProjectConvention,
        );
    }
    if signals.runtime_capability_style_required {
        return AuthoringStyleDecision::new(
            AuthoringStyle::ExistingProjectStyle,
            StyleAuthority::RuntimeCapability,
        );
    }
    if signals.model_robust_pytest_default {
        return AuthoringStyleDecision::new(
            AuthoringStyle::PytestFunctionStyle,
            StyleAuthority::ModelRobustDefault,
        );
    }
    match verifier_flavor {
        PythonProjectUnitVerifierFlavor::PytestStdlib
        | PythonProjectUnitVerifierFlavor::UnittestDiscover => {
            AuthoringStyleDecision::unspecified()
        }
    }
}

pub(super) fn decide_python_authoring_style_from_request(
    request: &str,
    is_python_contract: bool,
    requires_test_artifact: bool,
) -> AuthoringStyleDecision {
    if !is_python_contract || !requires_test_artifact {
        return AuthoringStyleDecision::unspecified();
    }
    let lower = request.to_ascii_lowercase();
    let explicit_unittest_requested =
        lower.contains("unittest") || lower.contains("python -m unittest");
    let explicit_pytest_requested = lower.contains("pytest");
    decide_python_authoring_style(
        PythonAuthoringStyleSignals {
            explicit_pytest_requested,
            explicit_unittest_requested,
            model_robust_pytest_default: !explicit_pytest_requested && !explicit_unittest_requested,
            ..PythonAuthoringStyleSignals::default()
        },
        PythonProjectUnitVerifierFlavor::PytestStdlib,
    )
}

pub(super) fn authoring_style_policy_note_for_runtime(
    runtime_profile: RuntimeProfile,
) -> &'static str {
    match runtime_profile {
        RuntimeProfile::Python => {
            "python_evidence_runner_is_not_test_authoring_style_for_ambiguous_new_python_tests_prefer_simple_pytest_function_style_unless_explicit_user_request_existing_unittest_tests_or_admitted_style_authority_requires_unittest_class"
        }
        RuntimeProfile::Node
        | RuntimeProfile::TypeScript
        | RuntimeProfile::Rust
        | RuntimeProfile::Unspecified => {
            "authoring_style_follows_objective_contract_and_existing_project_conventions_not_runner_command_shape"
        }
    }
}

pub(super) fn python_runner_style_mismatch_label(
    decision: AuthoringStyleDecision,
    verifier_flavor: PythonProjectUnitVerifierFlavor,
) -> &'static str {
    match (decision.style, verifier_flavor) {
        (
            AuthoringStyle::PytestFunctionStyle,
            PythonProjectUnitVerifierFlavor::UnittestDiscover,
        ) => "pytest_function_style_with_unittest_discover_runner",
        _ => "none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_unittest_request_can_select_unittest_authoring_style() {
        let decision = decide_python_authoring_style(
            PythonAuthoringStyleSignals {
                explicit_unittest_requested: true,
                ..PythonAuthoringStyleSignals::default()
            },
            PythonProjectUnitVerifierFlavor::PytestStdlib,
        );

        assert_eq!(
            decision,
            AuthoringStyleDecision::new(
                AuthoringStyle::UnittestClassStyle,
                StyleAuthority::ExplicitUserRequest
            )
        );
    }

    #[test]
    fn explicit_pytest_request_can_select_pytest_function_style() {
        let decision = decide_python_authoring_style(
            PythonAuthoringStyleSignals {
                explicit_pytest_requested: true,
                ..PythonAuthoringStyleSignals::default()
            },
            PythonProjectUnitVerifierFlavor::UnittestDiscover,
        );

        assert_eq!(
            decision,
            AuthoringStyleDecision::new(
                AuthoringStyle::PytestFunctionStyle,
                StyleAuthority::ExplicitUserRequest
            )
        );
    }

    #[test]
    fn existing_project_convention_is_not_runner_authority() {
        let decision = decide_python_authoring_style(
            PythonAuthoringStyleSignals {
                existing_unittest_convention: true,
                ..PythonAuthoringStyleSignals::default()
            },
            PythonProjectUnitVerifierFlavor::PytestStdlib,
        );

        assert_eq!(
            decision,
            AuthoringStyleDecision::new(
                AuthoringStyle::ExistingProjectStyle,
                StyleAuthority::ExistingProjectConvention
            )
        );
    }

    #[test]
    fn ambiguous_python_task_does_not_inherit_unittest_from_runner() {
        let decision = decide_python_authoring_style(
            PythonAuthoringStyleSignals::default(),
            PythonProjectUnitVerifierFlavor::UnittestDiscover,
        );

        assert_eq!(decision, AuthoringStyleDecision::unspecified());
    }

    #[test]
    fn model_robust_default_can_choose_pytest_without_explicit_runner_authority() {
        let decision = decide_python_authoring_style(
            PythonAuthoringStyleSignals {
                model_robust_pytest_default: true,
                ..PythonAuthoringStyleSignals::default()
            },
            PythonProjectUnitVerifierFlavor::UnittestDiscover,
        );

        assert_eq!(
            decision,
            AuthoringStyleDecision::new(
                AuthoringStyle::PytestFunctionStyle,
                StyleAuthority::ModelRobustDefault
            )
        );
    }

    #[test]
    fn runtime_capability_can_explain_style_without_using_runner_as_authority() {
        let decision = decide_python_authoring_style(
            PythonAuthoringStyleSignals {
                runtime_capability_style_required: true,
                ..PythonAuthoringStyleSignals::default()
            },
            PythonProjectUnitVerifierFlavor::UnittestDiscover,
        );

        assert_eq!(
            decision,
            AuthoringStyleDecision::new(
                AuthoringStyle::ExistingProjectStyle,
                StyleAuthority::RuntimeCapability
            )
        );
    }

    #[test]
    fn python_policy_note_keeps_runner_separate_from_authoring_style() {
        let note = authoring_style_policy_note_for_runtime(RuntimeProfile::Python);

        assert!(note.contains("evidence_runner_is_not_test_authoring_style"));
        assert!(note.contains("unless_explicit_user_request"));
        assert!(note.contains("prefer_simple_pytest_function_style"));
    }

    #[test]
    fn request_decision_prefers_pytest_for_ambiguous_python_tests() {
        let decision = decide_python_authoring_style_from_request(
            "Create main.py and tests/test_main.py. Add tests and run them.",
            true,
            true,
        );

        assert_eq!(
            decision,
            AuthoringStyleDecision::new(
                AuthoringStyle::PytestFunctionStyle,
                StyleAuthority::ModelRobustDefault
            )
        );
    }

    #[test]
    fn request_decision_preserves_explicit_unittest() {
        let decision = decide_python_authoring_style_from_request(
            "Create tests/test_main.py. Use Python unittest and run the tests.",
            true,
            true,
        );

        assert_eq!(
            decision,
            AuthoringStyleDecision::new(
                AuthoringStyle::UnittestClassStyle,
                StyleAuthority::ExplicitUserRequest
            )
        );
    }

    #[test]
    fn request_decision_does_not_style_non_python_contracts() {
        let decision = decide_python_authoring_style_from_request(
            "Create tests/test_main.py. Add tests and run them.",
            false,
            true,
        );

        assert_eq!(decision, AuthoringStyleDecision::unspecified());
    }

    #[test]
    fn python_runner_style_mismatch_flags_pytest_tests_under_unittest_runner() {
        let mismatch = python_runner_style_mismatch_label(
            AuthoringStyleDecision::new(
                AuthoringStyle::PytestFunctionStyle,
                StyleAuthority::ModelRobustDefault,
            ),
            PythonProjectUnitVerifierFlavor::UnittestDiscover,
        );
        assert_eq!(
            mismatch,
            "pytest_function_style_with_unittest_discover_runner"
        );

        let compatible = python_runner_style_mismatch_label(
            AuthoringStyleDecision::new(
                AuthoringStyle::UnittestClassStyle,
                StyleAuthority::ExplicitUserRequest,
            ),
            PythonProjectUnitVerifierFlavor::PytestStdlib,
        );
        assert_eq!(compatible, "none");
    }
}
