use crate::roles::{RoleDefinition, RoleRegistry};

pub fn public_model_override_roles(registry: &RoleRegistry) -> Vec<&RoleDefinition> {
    registry
        .roles
        .iter()
        .filter(|role| role.enabled_in_mvp && role.user_facing && role.supports_model_override && role.id != "pm")
        .collect()
}

pub fn user_facing_roles(registry: &RoleRegistry) -> Vec<&RoleDefinition> {
    registry
        .roles
        .iter()
        .filter(|role| role.enabled_in_mvp && role.user_facing && role.id != "pm")
        .collect()
}

pub fn persisted_roles(registry: &RoleRegistry) -> Vec<&RoleDefinition> {
    public_model_override_roles(registry)
}
