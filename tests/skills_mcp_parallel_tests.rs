use std::fs;

use anvil::mcp::client::McpRegistry;
use anvil::skills::loader::SkillLibrary;
use tempfile::tempdir;

#[test]
fn skill_library_lists_and_loads_markdown() {
    let dir = tempdir().unwrap();
    let skills_dir = dir.path().join("skills");
    fs::create_dir_all(&skills_dir).unwrap();
    fs::write(skills_dir.join("review.md"), "# Review\nUse review mode.").unwrap();

    let library = SkillLibrary::new(skills_dir);
    let skills = library.list().unwrap();
    assert_eq!(skills, vec!["review".to_string()]);
    let skill = library.load("review").unwrap();
    assert!(skill.content.contains("Use review mode"));
}

#[test]
fn mcp_registry_lists_server_status() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    fs::write(
        &path,
        r#"{"servers":[{"name":"docs","command":"python","args":["server.py"]}]}"#,
    )
    .unwrap();

    let registry = McpRegistry::new(path);
    let lines = registry.status_lines().unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("docs -> python server.py"));
}
