use std::fs;

use anvil::config::repo_instructions::RepoInstructions;
use anvil::prompts::context::ContextBlock;
use anvil::runtime::engine::RuntimeEngine;
use anvil::runtime::sandbox::SandboxPolicy;
use anvil::runtime::{trust::SourceType, NetworkPolicy, PermissionMode};
use anvil::tools::exec::ExecRequest;
use anvil::tools::registry::{ToolRegistry, ToolRequest, ToolResponse};
use tempfile::tempdir;

#[test]
fn repo_instructions_load_from_anvil_md() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("anvil.md");
    fs::write(&path, "# anvil.md\n\nPrefer minimal diffs\n").expect("write anvil.md");

    let instructions = RepoInstructions::load(temp.path()).expect("load instructions");

    assert!(instructions.is_present());
    assert_eq!(instructions.path.as_deref(), Some(path.as_path()));
    assert!(instructions
        .contents
        .as_deref()
        .expect("contents")
        .contains("Prefer minimal diffs"));
}

#[test]
fn runtime_engine_blocks_writes_in_read_only_mode() {
    let temp = tempdir().expect("tempdir");
    let sandbox = SandboxPolicy::new(
        PermissionMode::ReadOnly,
        NetworkPolicy::Disabled,
        temp.path().to_path_buf(),
        vec![],
    );
    let engine = RuntimeEngine::new(
        sandbox,
        ToolRegistry::default(),
        RepoInstructions::default(),
    );

    let event = engine
        .execute(ToolRequest::WriteFile {
            path: temp.path().join("note.txt"),
            contents: "hello".to_string(),
        })
        .expect("runtime event");

    assert!(event.message.contains("blocked"));
}

#[test]
fn runtime_engine_requires_confirmation_for_network_commands() {
    let temp = tempdir().expect("tempdir");
    let sandbox = SandboxPolicy::new(
        PermissionMode::WorkspaceWrite,
        NetworkPolicy::Disabled,
        temp.path().to_path_buf(),
        vec![],
    );
    let engine = RuntimeEngine::new(
        sandbox,
        ToolRegistry::default(),
        RepoInstructions::default(),
    );

    let event = engine
        .execute(ToolRequest::Exec {
            request: ExecRequest {
                program: "curl".to_string(),
                args: vec!["https://example.com".to_string()],
                cwd: temp.path().to_path_buf(),
            },
        })
        .expect("runtime event");

    assert!(event.message.contains("confirmation required"));
}

#[test]
fn tool_registry_reads_searches_and_inspects_env() {
    let temp = tempdir().expect("tempdir");
    let file = temp.path().join("notes.txt");
    fs::write(&file, "alpha\nbeta needle\ngamma\n").expect("write notes");

    let registry = ToolRegistry::default();

    match registry
        .execute(ToolRequest::ReadFile { path: file.clone() })
        .expect("read file")
    {
        ToolResponse::FileContents(result) => assert!(result.contents.contains("beta needle")),
        other => panic!("unexpected response: {other:?}"),
    }

    match registry
        .execute(ToolRequest::Search {
            root: temp.path().to_path_buf(),
            needle: "needle".to_string(),
        })
        .expect("search")
    {
        ToolResponse::SearchMatches(matches) => {
            assert_eq!(matches.len(), 1);
            assert_eq!(matches[0].path, file);
            assert_eq!(matches[0].line_number, 2);
        }
        other => panic!("unexpected response: {other:?}"),
    }

    match registry
        .execute(ToolRequest::InspectEnv)
        .expect("inspect env")
    {
        ToolResponse::EnvSnapshot(snapshot) => assert!(snapshot.cwd.is_absolute()),
        other => panic!("unexpected response: {other:?}"),
    }
}

#[test]
fn runtime_context_includes_anvil_md_between_user_and_repo_content() {
    let temp = tempdir().expect("tempdir");
    fs::write(
        temp.path().join("anvil.md"),
        "# anvil.md\n\nPrefer minimal diffs\n",
    )
    .expect("write anvil.md");
    let instructions = RepoInstructions::load(temp.path()).expect("load instructions");
    let sandbox = SandboxPolicy::new(
        PermissionMode::ReadOnly,
        NetworkPolicy::Disabled,
        temp.path().to_path_buf(),
        vec![],
    );
    let engine = RuntimeEngine::new(sandbox, ToolRegistry::default(), instructions);

    let context = engine.build_context(
        "Inspect the workspace",
        vec![ContextBlock::new(SourceType::RepoFile, "fn main() {}").with_path("src/main.rs")],
    );

    let user_index = context.find("[source=user]").expect("user");
    let anvil_md_index = context.find("[source=anvil-md").expect("anvil-md");
    let repo_index = context
        .find("[source=repo-file path=src/main.rs]")
        .expect("repo file");

    assert!(user_index < anvil_md_index);
    assert!(anvil_md_index < repo_index);
}
