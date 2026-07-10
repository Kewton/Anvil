use regex::Regex;

use crate::planner::profile::ProfileHookAttribute;

pub fn hook_attributes_present(source: &str, attributes: &[ProfileHookAttribute]) -> bool {
    attributes
        .iter()
        .all(|attribute| hook_attribute_present(source, *attribute))
}

pub fn hook_attribute_present(source: &str, attribute: ProfileHookAttribute) -> bool {
    match attribute {
        ProfileHookAttribute::PrimaryAction => data_anvil_action_present(source, "primary"),
        ProfileHookAttribute::RestartAction => data_anvil_action_present(source, "restart"),
        ProfileHookAttribute::State => data_anvil_state_present(source),
    }
}

pub fn data_anvil_action_present(source: &str, value: &str) -> bool {
    let pattern = format!(
        r#"(?s)data-anvil-action\s*=\s*(?:"{0}"|'{0}'|\{{\s*(?:"{0}"|'{0}'|`{0}`)\s*\}})"#,
        regex::escape(value)
    );
    Regex::new(&pattern).is_ok_and(|regex| regex.is_match(source))
}

pub fn data_anvil_state_present(source: &str) -> bool {
    Regex::new(r#"(?s)data-anvil-state(?:\s*=|\b)"#).is_ok_and(|regex| regex.is_match(source))
}
