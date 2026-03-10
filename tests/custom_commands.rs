use anvil::slash::custom::{
    CustomAction, CustomCommandDefinition, CustomCommandInvocation, CustomExecutionContext,
    load_custom_commands,
};
use tempfile::tempdir;

#[test]
fn loads_schema_backed_custom_command_from_toml() {
    let dir = tempdir().unwrap();
    let commands_dir = dir.path().join(".anvil/commands");
    std::fs::create_dir_all(&commands_dir).unwrap();
    std::fs::write(
        commands_dir.join("remember.toml"),
        r#"
name = "remember"
description = "append a memory entry"
action = "memory_add"

[[args]]
name = "text"
type = "string"
required = true
"#,
    )
    .unwrap();

    let defs = load_custom_commands(dir.path()).unwrap();

    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].name, "remember");
    assert_eq!(defs[0].action, CustomAction::MemoryAdd);
}

#[test]
fn custom_command_validate_and_invoke_memory_action() {
    let dir = tempdir().unwrap();
    let memory_path = dir.path().join("ANVIL-MEMORY.md");
    let def = CustomCommandDefinition::from_toml_str(
        r#"
name = "remember"
description = "append a memory entry"
action = "memory_add"

[[args]]
name = "text"
type = "string"
required = true
"#,
    )
    .unwrap();

    let invocation = CustomCommandInvocation::parse(&def, "/remember text='Prefer short replies'")
        .unwrap()
        .unwrap();
    invocation
        .execute(&CustomExecutionContext {
            memory_path: memory_path.clone(),
        })
        .unwrap();

    let memory = std::fs::read_to_string(memory_path).unwrap();
    assert!(memory.contains("Prefer short replies"));
}

#[test]
fn custom_command_rejects_unknown_argument_schema_bypass() {
    let def = CustomCommandDefinition::from_toml_str(
        r#"
name = "remember"
description = "append a memory entry"
action = "memory_add"

[[args]]
name = "text"
type = "string"
required = true
"#,
    )
    .unwrap();

    let err = CustomCommandInvocation::parse(&def, "/remember text=ok extra=bad").unwrap_err();
    assert!(format!("{err}").contains("unknown argument"));
}
