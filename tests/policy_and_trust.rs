use std::path::PathBuf;

use anvil::policy::command_classification::{classify_command, CommandClass};
use anvil::policy::network_policy::NetworkAccessPolicy;
use anvil::policy::path_policy::PathPolicy;
use anvil::prompts::context::{render_context_blocks, ContextBlock};
use anvil::runtime::{trust::SourceType, NetworkPolicy};

#[test]
fn command_classification_matches_mvp_expectations() {
    assert!(matches!(
        classify_command("rg", &[]),
        CommandClass::SafeRead
    ));
    assert!(matches!(
        classify_command("cargo", &[String::from("test")]),
        CommandClass::LocalValidation
    ));
    assert!(matches!(
        classify_command("curl", &[String::from("https://example.com")]),
        CommandClass::Networked
    ));
    assert!(matches!(
        classify_command("git", &[String::from("reset"), String::from("--hard")]),
        CommandClass::Destructive
    ));
}

#[test]
fn network_policy_distinguishes_local_and_remote_urls() {
    let local_only = NetworkAccessPolicy::new(NetworkPolicy::LocalOnly);

    assert!(local_only
        .allows_url("http://127.0.0.1:11434/api/generate")
        .expect("valid local url"));
    assert!(!local_only
        .allows_url("https://example.com")
        .expect("valid remote url"));
}

#[test]
fn path_policy_allows_workspace_writes_only_inside_roots() {
    let workspace = PathBuf::from("/tmp/anvil-workspace");
    let extra = PathBuf::from("/tmp/anvil-cache");
    let policy = PathPolicy::new(workspace.clone(), vec![extra.clone()]);

    assert!(policy.allows_write(&workspace.join("src/main.rs")));
    assert!(policy.allows_write(&extra.join("build.log")));
    assert!(!policy.allows_write(&PathBuf::from("/etc/hosts")));
}

#[test]
fn prompt_context_is_rendered_in_trust_order() {
    let rendered = render_context_blocks(&[
        ContextBlock::new(SourceType::RepoFile, "fn main() {}").with_path("src/main.rs"),
        ContextBlock::new(SourceType::User, "Implement the CLI"),
        ContextBlock::new(SourceType::AnvilMd, "Prefer minimal diffs"),
    ]);

    let user_index = rendered.find("[source=user]").expect("user block");
    let anvil_md_index = rendered.find("[source=anvil-md]").expect("anvil-md block");
    let repo_index = rendered
        .find("[source=repo-file path=src/main.rs]")
        .expect("repo-file block");

    assert!(user_index < anvil_md_index);
    assert!(anvil_md_index < repo_index);
}
