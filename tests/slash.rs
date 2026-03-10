use anvil::slash::builtins::{BuiltinCommand, parse_builtin_command};
use tempfile::tempdir;

#[test]
fn parses_memory_show_and_edit_commands() {
    assert_eq!(
        parse_builtin_command("/memory show").unwrap(),
        BuiltinCommand::MemoryShow
    );
    assert_eq!(
        parse_builtin_command("/memory edit prefer concise").unwrap(),
        BuiltinCommand::MemoryEdit {
            text: "prefer concise".to_string()
        }
    );
}

#[test]
fn builtin_memory_commands_update_store() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ANVIL-MEMORY.md");

    BuiltinCommand::MemoryAdd {
        text: "Keep replies tight".to_string(),
    }
    .execute(&path)
    .unwrap();
    let shown = BuiltinCommand::MemoryShow.execute(&path).unwrap();
    assert!(shown.contains("Keep replies tight"));

    BuiltinCommand::MemoryEdit {
        text: "Use short paragraphs".to_string(),
    }
    .execute(&path)
    .unwrap();
    let updated = BuiltinCommand::MemoryShow.execute(&path).unwrap();
    assert!(updated.contains("Use short paragraphs"));
    assert!(!updated.contains("Keep replies tight"));
}
