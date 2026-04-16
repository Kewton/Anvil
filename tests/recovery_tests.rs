use anvil::agent::recovery::{
    empty_response_recovery_note, no_tool_recovery_note, user_prompt_requires_action,
};
use anvil::modes::plan_act::ExecutionMode;

#[test]
fn detects_action_prompts_in_english_and_japanese() {
    assert!(user_prompt_requires_action(
        "Please edit src/main.rs and fix the bug",
        ExecutionMode::Act
    ));
    assert!(user_prompt_requires_action(
        "README を修正して",
        ExecutionMode::Act
    ));
    assert!(!user_prompt_requires_action(
        "Explain the architecture",
        ExecutionMode::Act
    ));
    assert!(!user_prompt_requires_action(
        "plan the work",
        ExecutionMode::Plan
    ));
}

#[test]
fn recovery_notes_are_non_empty() {
    assert!(empty_response_recovery_note(1, true).contains("attempt=1"));
    assert!(no_tool_recovery_note(2).contains("no_tool_attempt=2"));
}
