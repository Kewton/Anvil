use anvil::roles::{persisted_roles, public_model_override_roles, RoleRegistry};

#[test]
fn builtin_role_registry_loads() {
    let registry = RoleRegistry::load_builtin().expect("registry should load");
    assert_eq!(registry.default_session_role, "pm");
    assert!(registry.role("pm").is_some());
}

#[test]
fn public_override_roles_match_mvp_expectation() {
    let registry = RoleRegistry::load_builtin().expect("registry should load");
    let roles: Vec<_> = public_model_override_roles(&registry)
        .into_iter()
        .map(|role| role.id.as_str())
        .collect();

    assert_eq!(roles, vec!["reader", "editor", "tester", "reviewer"]);
}

#[test]
fn persisted_roles_match_public_override_roles() {
    let registry = RoleRegistry::load_builtin().expect("registry should load");
    let roles: Vec<_> = persisted_roles(&registry)
        .into_iter()
        .map(|role| role.id.as_str())
        .collect();

    assert_eq!(roles, vec!["reader", "editor", "tester", "reviewer"]);
}
