//! Tests for src/agent/tag_spec.rs — tag-based tool syntax definition table.

use anvil::agent::tag_spec::{TOOL_TAG_SPECS, find_spec};

#[test]
fn tool_tag_specs_has_nine_entries() {
    assert_eq!(TOOL_TAG_SPECS.len(), 9);
}

#[test]
fn find_spec_file_read() {
    let spec = find_spec("file.read").expect("file.read should exist");
    assert_eq!(spec.name, "file.read");
    assert_eq!(spec.attributes, &["path"]);
    assert!(spec.child_elements.is_empty());
    assert!(!spec.example.is_empty());
}

#[test]
fn find_spec_file_write() {
    let spec = find_spec("file.write").expect("file.write should exist");
    assert_eq!(spec.name, "file.write");
    assert_eq!(spec.attributes, &["path"]);
    assert_eq!(spec.child_elements, &["content"]);
}

#[test]
fn find_spec_file_edit() {
    let spec = find_spec("file.edit").expect("file.edit should exist");
    assert_eq!(spec.name, "file.edit");
    assert_eq!(spec.attributes, &["path"]);
    assert_eq!(spec.child_elements, &["old_string", "new_string"]);
}

#[test]
fn find_spec_file_search() {
    let spec = find_spec("file.search").expect("file.search should exist");
    assert_eq!(spec.attributes, &["root", "pattern"]);
    assert!(spec.child_elements.is_empty());
}

#[test]
fn find_spec_shell_exec() {
    let spec = find_spec("shell.exec").expect("shell.exec should exist");
    assert_eq!(spec.attributes, &["command"]);
    assert!(spec.child_elements.is_empty());
}

#[test]
fn find_spec_web_fetch() {
    let spec = find_spec("web.fetch").expect("web.fetch should exist");
    assert_eq!(spec.attributes, &["url"]);
    assert!(spec.child_elements.is_empty());
}

#[test]
fn find_spec_web_search() {
    let spec = find_spec("web.search").expect("web.search should exist");
    assert_eq!(spec.attributes, &["query"]);
    assert!(spec.child_elements.is_empty());
}

#[test]
fn find_spec_agent_explore() {
    let spec = find_spec("agent.explore").expect("agent.explore should exist");
    assert_eq!(spec.attributes, &["scope"]);
    assert_eq!(spec.child_elements, &["prompt"]);
}

#[test]
fn find_spec_agent_plan() {
    let spec = find_spec("agent.plan").expect("agent.plan should exist");
    assert_eq!(spec.attributes, &["scope"]);
    assert_eq!(spec.child_elements, &["prompt"]);
}

#[test]
fn find_spec_unknown_returns_none() {
    assert!(find_spec("unknown").is_none());
}

#[test]
fn find_spec_mcp_tool_returns_none() {
    assert!(find_spec("mcp__server__tool").is_none());
}
