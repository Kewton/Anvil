mod common;

use anvil::retrieval::{RepositoryIndex, render_retrieval_result};
use std::fs;

#[test]
fn repository_index_finds_matches_by_path_and_content() {
    let root = common::unique_test_dir("retrieval");
    fs::create_dir_all(root.join("src")).expect("src dir");
    fs::create_dir_all(root.join("docs")).expect("docs dir");
    fs::write(
        root.join("src/invader.rs"),
        "pub fn build_invader() { println!(\"space invader\"); }\n",
    )
    .expect("write src");
    fs::write(
        root.join("docs/game-notes.md"),
        "This document describes the space invader wave logic.\n",
    )
    .expect("write docs");

    let index = RepositoryIndex::build(&root).expect("index should build");
    let result = index.search("invader", 5);

    assert!(!result.matches.is_empty());
    assert_eq!(result.matches[0].path, "src/invader.rs");
    assert!(result.matches.iter().any(|item| {
        item.path == "docs/game-notes.md"
            && item
                .snippets
                .iter()
                .any(|snippet| snippet.contains("space invader"))
    }));
}

#[test]
fn retrieval_result_renders_operator_console_frame() {
    let root = common::unique_test_dir("retrieval_render");
    fs::create_dir_all(root.join("src")).expect("src dir");
    fs::write(root.join("src/app.rs"), "fn repo_find() {}\n").expect("write file");

    let index = RepositoryIndex::build(&root).expect("index should build");
    let rendered = render_retrieval_result(&index.search("repo_find", 5));

    assert!(rendered.contains("[A] anvil > repo-find repo_find"));
    assert!(rendered.contains("src/app.rs"));
}

#[test]
fn repository_index_can_persist_and_reload_cache() {
    let root = common::unique_test_dir("retrieval_cache");
    let state_dir = root.join(".anvil/state");
    fs::create_dir_all(root.join("src")).expect("src dir");
    fs::write(root.join("src/app.rs"), "fn repo_find() {}\n").expect("write file");

    let cache_path = anvil::retrieval::default_cache_path(&state_dir);
    let built = RepositoryIndex::load_or_build(&root, &cache_path).expect("cache build");
    let loaded = RepositoryIndex::load_or_build(&root, &cache_path).expect("cache load");

    assert_eq!(built.search("repo_find", 5), loaded.search("repo_find", 5));
    assert!(cache_path.exists());
}

#[test]
fn repository_index_invalidates_cache_when_repository_changes() {
    let root = common::unique_test_dir("retrieval_cache_invalidate");
    let state_dir = root.join(".anvil/state");
    fs::create_dir_all(root.join("src")).expect("src dir");
    fs::write(root.join("src/app.rs"), "fn repo_find() {}\n").expect("write file");

    let cache_path = anvil::retrieval::default_cache_path(&state_dir);
    let _ = RepositoryIndex::load_or_build(&root, &cache_path).expect("cache build");

    fs::write(
        root.join("src/new_feature.rs"),
        "pub fn invalidate_cache() -> bool { true }\n",
    )
    .expect("write changed file");

    let updated = RepositoryIndex::load_or_build(&root, &cache_path).expect("cache reload");
    let result = updated.search("invalidate_cache", 5);

    assert_eq!(result.matches[0].path, "src/new_feature.rs");
}

#[test]
fn retrieval_scoring_prefers_file_name_match_over_content_only_match() {
    let root = common::unique_test_dir("retrieval_scoring");
    fs::create_dir_all(root.join("src")).expect("src dir");
    fs::create_dir_all(root.join("docs")).expect("docs dir");
    fs::write(
        root.join("src/retrieval_score.rs"),
        "pub fn unrelated() {}\n",
    )
    .expect("write path match");
    fs::write(
        root.join("docs/notes.md"),
        "this mentions retrieval_score in prose only\nthis mentions retrieval_score again\n",
    )
    .expect("write content match");

    let index = RepositoryIndex::build(&root).expect("index should build");
    let result = index.search("retrieval_score", 5);

    assert_eq!(result.matches[0].path, "src/retrieval_score.rs");
    assert!(result.matches[0].score > result.matches[1].score);
}
