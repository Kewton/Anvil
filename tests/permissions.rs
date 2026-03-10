use anvil::policy::permissions::{
    ExecutionContext, InteractionMode, NonInteractiveBehavior, PermissionCategory, PermissionMode,
    PermissionPolicy, PermissionRequirement,
};

#[derive(Debug)]
struct Case {
    mode: PermissionMode,
    category: PermissionCategory,
    cx: ExecutionContext,
    expected_base: PermissionRequirement,
    expected_effective: PermissionRequirement,
}

#[test]
fn permission_policy_table_driven() {
    let interactive = ExecutionContext {
        interaction_mode: InteractionMode::Interactive,
        non_interactive_ask: NonInteractiveBehavior::Deny,
        non_interactive_soft_confirm: NonInteractiveBehavior::Deny,
        non_interactive_hard_confirm: NonInteractiveBehavior::Deny,
    };
    let non_interactive_permissive = ExecutionContext {
        interaction_mode: InteractionMode::NonInteractive,
        non_interactive_ask: NonInteractiveBehavior::Allow,
        non_interactive_soft_confirm: NonInteractiveBehavior::Allow,
        non_interactive_hard_confirm: NonInteractiveBehavior::Allow,
    };

    let cases = vec![
        Case {
            mode: PermissionMode::Ask,
            category: PermissionCategory::Read,
            cx: interactive,
            expected_base: PermissionRequirement::Allow,
            expected_effective: PermissionRequirement::Allow,
        },
        Case {
            mode: PermissionMode::BypassPermissions,
            category: PermissionCategory::ExecDangerous,
            cx: non_interactive_permissive,
            expected_base: PermissionRequirement::HardConfirm,
            expected_effective: PermissionRequirement::Deny,
        },
        Case {
            mode: PermissionMode::BypassPermissions,
            category: PermissionCategory::ExecSensitive,
            cx: non_interactive_permissive,
            expected_base: PermissionRequirement::SoftConfirm,
            expected_effective: PermissionRequirement::Allow,
        },
    ];

    for case in cases {
        let policy = PermissionPolicy::from_mode(case.mode, case.category);
        assert_eq!(policy.base_requirement(), case.expected_base);
        assert_eq!(
            policy.effective_requirement(case.cx),
            case.expected_effective
        );
    }
}
