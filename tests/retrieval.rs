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

#[test]
fn cache_invalidates_on_file_rename() {
    let root = common::unique_test_dir("retrieval_rename");
    let state_dir = root.join(".anvil/state");
    fs::create_dir_all(root.join("src")).expect("src dir");
    fs::write(
        root.join("src/alpha.rs"),
        "pub fn unique_rename_marker() {}\n",
    )
    .expect("write alpha");
    fs::write(root.join("src/beta.rs"), "pub fn other() {}\n").expect("write beta");

    let cache_path = anvil::retrieval::default_cache_path(&state_dir);
    let _ = RepositoryIndex::load_or_build(&root, &cache_path).expect("initial build");

    // Remove alpha, create gamma with identical content/size but different name
    let content = fs::read_to_string(root.join("src/alpha.rs")).expect("read alpha");
    fs::remove_file(root.join("src/alpha.rs")).expect("remove alpha");
    fs::write(root.join("src/gamma.rs"), &content).expect("write gamma");

    let updated = RepositoryIndex::load_or_build(&root, &cache_path).expect("reload");
    let result = updated.search("unique_rename_marker", 5);

    assert!(
        !result.matches.is_empty(),
        "renamed file should be found after cache invalidation"
    );
    assert_eq!(result.matches[0].path, "src/gamma.rs");
}

#[test]
fn cache_invalidates_on_content_swap() {
    let root = common::unique_test_dir("retrieval_swap");
    let state_dir = root.join(".anvil/state");
    fs::create_dir_all(root.join("src")).expect("src dir");

    // Two files with different sizes so swapping changes per-file sizes but total stays same
    fs::write(root.join("src/short.rs"), "fn a() {}\n").expect("write short");
    fs::write(
        root.join("src/long.rs"),
        "fn swap_detection_content() { let x = 42; }\n",
    )
    .expect("write long");

    let cache_path = anvil::retrieval::default_cache_path(&state_dir);
    let _ = RepositoryIndex::load_or_build(&root, &cache_path).expect("initial build");

    // Swap contents
    let short_content = fs::read_to_string(root.join("src/short.rs")).expect("read short");
    let long_content = fs::read_to_string(root.join("src/long.rs")).expect("read long");
    fs::write(root.join("src/short.rs"), &long_content).expect("swap to short");
    fs::write(root.join("src/long.rs"), &short_content).expect("swap to long");

    let updated = RepositoryIndex::load_or_build(&root, &cache_path).expect("reload");
    let result = updated.search("swap_detection_content", 5);

    assert!(
        !result.matches.is_empty(),
        "swapped content should be found after cache invalidation"
    );
    assert_eq!(
        result.matches[0].path, "src/short.rs",
        "content should now be in short.rs after swap"
    );
}

#[test]
fn cache_invalidates_on_older_file_modification() {
    let root = common::unique_test_dir("retrieval_older_mod");
    let state_dir = root.join(".anvil/state");
    fs::create_dir_all(root.join("src")).expect("src dir");

    // Create file A first (older mtime)
    fs::write(root.join("src/older.rs"), "fn placeholder() {}\n").expect("write older");

    // Small delay to ensure different mtime
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Create file B second (newer mtime — this will be max_mtime)
    fs::write(root.join("src/newer.rs"), "fn newer_file_content() {}\n").expect("write newer");

    let cache_path = anvil::retrieval::default_cache_path(&state_dir);
    let _ = RepositoryIndex::load_or_build(&root, &cache_path).expect("initial build");

    // Modify the OLDER file — max_mtime stays same under aggregate hash
    // but per-entry hash detects the change
    std::thread::sleep(std::time::Duration::from_millis(50));
    fs::write(
        root.join("src/older.rs"),
        "fn older_file_modified_marker() { let changed = true; }\n",
    )
    .expect("modify older");

    let updated = RepositoryIndex::load_or_build(&root, &cache_path).expect("reload");
    let result = updated.search("older_file_modified_marker", 5);

    assert!(
        !result.matches.is_empty(),
        "modified older file content should be found after cache invalidation"
    );
    assert_eq!(result.matches[0].path, "src/older.rs");
}
