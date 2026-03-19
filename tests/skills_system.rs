mod common;

use anvil::extensions::skills::{self, SkillScope};
use anvil::extensions::{ExtensionRegistry, SlashCommandAction, SlashCommandSpec};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_test_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_nanos();
    std::env::temp_dir().join(format!("anvil_skill_test_{label}_{nanos}"))
}

fn write_skill_md(base: &std::path::Path, skill_name: &str, content: &str) {
    let skill_dir = base.join(".anvil").join("skills").join(skill_name);
    fs::create_dir_all(&skill_dir).expect("create skill dir");
    fs::write(skill_dir.join("SKILL.md"), content).expect("write SKILL.md");
}

fn valid_skill_md(name: &str) -> String {
    format!(
        r#"---
name: {name}
description: A test skill
argument-hint: <arg>
user-invocable: true
disable-auto-invocation: true
---

This is the skill body with $ARGUMENTS and ${{ANVIL_SKILL_DIR}}.
"#
    )
}

// --- Test 1: skill_md_parse_valid ---
#[test]
fn skill_md_parse_valid() {
    let content = r#"---
name: my-skill
description: A useful skill
argument-hint: <file>
user-invocable: true
disable-auto-invocation: false
---

Do something with $ARGUMENTS.
"#;
    let (fm, body) = skills::parse_frontmatter(content).expect("should parse valid frontmatter");
    assert_eq!(fm.name, "my-skill");
    assert_eq!(fm.description, "A useful skill");
    assert_eq!(fm.argument_hint, "<file>");
    assert!(fm.user_invocable);
    assert!(!fm.disable_auto_invocation);
    assert!(body.contains("Do something with $ARGUMENTS."));
}

// --- Test 2: skill_md_parse_missing_required ---
#[test]
fn skill_md_parse_missing_required() {
    // Missing description
    let content = r#"---
name: incomplete
---

Body content.
"#;
    let result = skills::parse_frontmatter(content);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .contains("missing required field: description")
    );

    // Missing name
    let content2 = r#"---
description: no name skill
---

Body content.
"#;
    let result2 = skills::parse_frontmatter(content2);
    assert!(result2.is_err());
    assert!(
        result2
            .unwrap_err()
            .contains("missing required field: name")
    );
}

// --- Test 3: skill_md_parse_invalid_yaml ---
#[test]
fn skill_md_parse_invalid_yaml() {
    // No opening ---
    let content = "name: bad\ndescription: no delimiters\n";
    let result = skills::parse_frontmatter(content);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .contains("frontmatter must start with ---")
    );

    // No closing ---
    let content2 = "---\nname: bad\ndescription: no close\n";
    let result2 = skills::parse_frontmatter(content2);
    assert!(result2.is_err());
    assert!(result2.unwrap_err().contains("closing --- not found"));
}

// --- Test 4: skill_md_name_dir_mismatch ---
#[test]
fn skill_md_name_dir_mismatch() {
    let root = unique_test_dir("name_mismatch");
    let skill_dir = root.join(".anvil").join("skills").join("wrong-name");
    fs::create_dir_all(&skill_dir).expect("create skill dir");
    fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
name: correct-name
description: Mismatch test
---

Body.
"#,
    )
    .expect("write SKILL.md");

    let result = skills::discover_and_load(&root, None, &[]);
    // Should be empty because name doesn't match directory
    assert!(result.is_empty());
}

// --- Test 5: skill_discover_user_scope ---
#[test]
fn skill_discover_user_scope() {
    let home = unique_test_dir("user_scope_home");
    write_skill_md(&home, "user-tool", &valid_skill_md("user-tool"));

    let cwd = unique_test_dir("user_scope_cwd");
    fs::create_dir_all(&cwd).expect("create cwd");

    let result = skills::discover_and_load(&cwd, Some(&home), &[]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "/user-tool");
    assert_eq!(result[0].scope, Some(SkillScope::User));
}

// --- Test 6: skill_discover_project_scope ---
#[test]
fn skill_discover_project_scope() {
    let root = unique_test_dir("project_scope");
    write_skill_md(&root, "proj-tool", &valid_skill_md("proj-tool"));

    let result = skills::discover_and_load(&root, None, &[]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "/proj-tool");
    assert_eq!(result[0].scope, Some(SkillScope::Project));
}

// --- Test 7: skill_scope_priority ---
#[test]
fn skill_scope_priority() {
    let home = unique_test_dir("scope_prio_home");
    let cwd = unique_test_dir("scope_prio_cwd");

    write_skill_md(&home, "shared-skill", &valid_skill_md("shared-skill"));
    write_skill_md(&cwd, "shared-skill", &valid_skill_md("shared-skill"));

    let result = skills::discover_and_load(&cwd, Some(&home), &[]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "/shared-skill");
    // Project scope should override user scope
    assert_eq!(result[0].scope, Some(SkillScope::Project));
}

// --- Test 8: skill_name_collision_builtin ---
#[test]
fn skill_name_collision_builtin() {
    let root = unique_test_dir("collision_builtin");
    // "help" conflicts with /help builtin
    write_skill_md(&root, "help", &valid_skill_md("help"));

    let builtin = anvil::extensions::builtin_slash_commands();
    let result = skills::discover_and_load(&root, None, &builtin);
    // Should be skipped due to collision
    assert!(result.is_empty());
}

// --- Test 9: skill_name_collision_custom ---
#[test]
fn skill_name_collision_custom() {
    let root = unique_test_dir("collision_custom");
    write_skill_md(&root, "invaders", &valid_skill_md("invaders"));

    let existing = vec![SlashCommandSpec {
        name: "/invaders".to_string(),
        description: "custom command".to_string(),
        action: SlashCommandAction::Prompt("do something".to_string()),
        scope: None,
    }];

    let result = skills::discover_and_load(&root, None, &existing);
    assert!(result.is_empty());
}

// --- Test 10: skill_args_separation ---
#[test]
fn skill_args_separation() {
    let commands = vec![SlashCommandSpec {
        name: "/my-skill".to_string(),
        description: "test skill".to_string(),
        action: SlashCommandAction::Skill {
            name: "my-skill".to_string(),
            args: String::new(),
            content: "skill body".to_string(),
            skill_dir: PathBuf::from("/tmp/skills/my-skill"),
        },
        scope: Some(SkillScope::User),
    }];

    let result = skills::parse_skill_command("/my-skill arg1 arg2", &commands);
    assert!(result.is_some());
    let spec = result.unwrap();
    assert_eq!(spec.name, "/my-skill");
    match &spec.action {
        SlashCommandAction::Skill { args, .. } => {
            assert_eq!(args, "arg1 arg2");
        }
        _ => panic!("expected Skill action"),
    }
}

// --- Test 11: skill_variable_expansion_arguments ---
#[test]
fn skill_variable_expansion_arguments() {
    let content = "Run with $ARGUMENTS and also ${ARGUMENTS} here.";
    let expanded = skills::expand_variables(content, "hello world", std::path::Path::new("/tmp"));
    assert_eq!(expanded, "Run with hello world and also hello world here.");
}

// --- Test 12: skill_variable_expansion_skill_dir ---
#[test]
fn skill_variable_expansion_skill_dir() {
    let content = "Dir is ${ANVIL_SKILL_DIR}/templates.";
    let expanded = skills::expand_variables(
        content,
        "",
        std::path::Path::new("/home/user/.anvil/skills/my-skill"),
    );
    assert_eq!(
        expanded,
        "Dir is /home/user/.anvil/skills/my-skill/templates."
    );
}

// --- Test 13: skill_user_invocable_false_skip ---
#[test]
fn skill_user_invocable_false_skip() {
    let root = unique_test_dir("not_invocable");
    let skill_dir = root.join(".anvil").join("skills").join("hidden");
    fs::create_dir_all(&skill_dir).expect("create skill dir");
    fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
name: hidden
description: Not user invocable
user-invocable: false
---

Hidden body.
"#,
    )
    .expect("write SKILL.md");

    let result = skills::discover_and_load(&root, None, &[]);
    assert!(result.is_empty());
}

// --- Test 14: skill_parse_error_skip_continue ---
#[test]
fn skill_parse_error_skip_continue() {
    let root = unique_test_dir("parse_error_skip");

    // Valid skill
    write_skill_md(&root, "good-skill", &valid_skill_md("good-skill"));

    // Invalid skill (no frontmatter)
    let bad_dir = root.join(".anvil").join("skills").join("bad-skill");
    fs::create_dir_all(&bad_dir).expect("create dir");
    fs::write(bad_dir.join("SKILL.md"), "no frontmatter here").expect("write bad skill");

    let result = skills::discover_and_load(&root, None, &[]);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "/good-skill");
}

// --- Test 15: skill_suggest_command_typo_correction ---
#[test]
fn skill_suggest_command_typo_correction() {
    let root = unique_test_dir("typo_correction");
    write_skill_md(&root, "deploy", &valid_skill_md("deploy"));

    let registry = ExtensionRegistry::load(&root, None).expect("load should succeed");
    // "/deplpy" is a typo for "/deploy" (edit distance 2)
    let suggestion = registry.suggest_command("/deplpy");
    assert_eq!(suggestion, Some("/deploy"));
}

// --- Test 16: skill_discover_no_slash_commands_json ---
#[test]
fn skill_discover_no_slash_commands_json() {
    let root = unique_test_dir("no_slash_json");
    write_skill_md(&root, "standalone", &valid_skill_md("standalone"));
    // No .anvil/slash-commands.json exists

    let registry = ExtensionRegistry::load(&root, None).expect("load should succeed");
    let commands = registry.slash_commands();
    assert!(commands.iter().any(|c| c.name == "/standalone"));
}
