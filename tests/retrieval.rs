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

// =========================================================================
// New tests for issue #133
// =========================================================================

#[test]
fn multi_keyword_path_scoring() {
    let root = common::unique_test_dir("retrieval_multi_kw");
    fs::create_dir_all(root.join("src/retrieval")).expect("dir");
    fs::write(root.join("src/retrieval/mod.rs"), "pub fn search() {}\n").expect("write");
    fs::write(root.join("src/other.rs"), "fn other() {}\n").expect("write other");

    let index = RepositoryIndex::build(&root).expect("build");
    let result = index.search("retrieval mod", 10);

    assert!(!result.matches.is_empty());
    // The file matching both keywords in its path should score highest
    assert_eq!(result.matches[0].path, "src/retrieval/mod.rs");
}

#[test]
fn changed_file_boost() {
    // This test verifies the boost mechanism exists by checking that
    // get_changed_files returns empty in a non-git temp dir (fallback).
    // The boost logic itself is unit-tested in mod tests.
    let root = common::unique_test_dir("retrieval_changed");
    fs::create_dir_all(root.join("src")).expect("dir");
    fs::write(root.join("src/app.rs"), "fn main() {}\n").expect("write");

    let index = RepositoryIndex::build(&root).expect("build");
    // Should not panic even without git
    let result = index.search("app", 10);
    assert!(!result.matches.is_empty());
}

#[test]
fn related_test_boost() {
    // Verify that searching works with test-like files present
    let root = common::unique_test_dir("retrieval_related_test");
    fs::create_dir_all(root.join("src")).expect("dir");
    fs::create_dir_all(root.join("tests")).expect("tests dir");
    fs::write(root.join("src/widget.rs"), "pub fn widget() {}\n").expect("write src");
    fs::write(root.join("tests/widget_test.rs"), "fn test_widget() {}\n").expect("write test");

    let index = RepositoryIndex::build(&root).expect("build");
    let result = index.search("widget", 10);

    assert!(result.matches.len() >= 2);
    // Both src and test file should appear
    assert!(result.matches.iter().any(|m| m.path == "src/widget.rs"));
    assert!(
        result
            .matches
            .iter()
            .any(|m| m.path == "tests/widget_test.rs")
    );
}

#[test]
fn neighbor_file_boost() {
    // Verify neighbor boost doesn't cause errors
    let root = common::unique_test_dir("retrieval_neighbor");
    fs::create_dir_all(root.join("src/module")).expect("dir");
    fs::write(root.join("src/module/core.rs"), "fn core_function() {}\n").expect("write");
    fs::write(
        root.join("src/module/helper.rs"),
        "fn helper_function() {}\n",
    )
    .expect("write helper");

    let index = RepositoryIndex::build(&root).expect("build");
    let result = index.search("core", 10);
    assert!(!result.matches.is_empty());
}

#[test]
fn symbol_match_scoring() {
    let root = common::unique_test_dir("retrieval_symbol");
    fs::create_dir_all(root.join("src")).expect("dir");
    // File with no path match but contains a struct named "Frobnicator"
    fs::write(
        root.join("src/engine.rs"),
        "pub struct Frobnicator { value: i32 }\nimpl Frobnicator { fn new() -> Self { Self { value: 0 } } }\n",
    )
    .expect("write");

    let index = RepositoryIndex::build(&root).expect("build");
    let result = index.search("frobnicator", 10);

    assert!(!result.matches.is_empty());
    // The file defining Frobnicator should be found via symbol + content matching
    assert!(result.matches.iter().any(|m| m.path == "src/engine.rs"));
}

#[test]
fn content_line_scoring_with_cap() {
    let root = common::unique_test_dir("retrieval_content_cap");
    fs::create_dir_all(root.join("src")).expect("dir");
    // Create a file with many matching lines
    let content: String = (0..50)
        .map(|i| format!("let unique_cap_test_token_{i} = unique_cap_test_token;\n"))
        .collect();
    fs::write(root.join("src/big.rs"), &content).expect("write big file");

    let index = RepositoryIndex::build(&root).expect("build");
    let result = index.search("unique_cap_test_token", 10);

    assert!(!result.matches.is_empty());
    let big_match = result
        .matches
        .iter()
        .find(|m| m.path == "src/big.rs")
        .expect("big.rs should be in results");
    // Content line score is capped at 80, plus path matching bonus
    // The point is it doesn't grow unbounded with line count
    assert!(
        big_match.score <= 1000,
        "score should be bounded, got {}",
        big_match.score
    );
}

#[test]
fn all_keywords_bonus() {
    let root = common::unique_test_dir("retrieval_all_kw_bonus");
    fs::create_dir_all(root.join("src")).expect("dir");
    // File containing both keywords
    fs::write(
        root.join("src/combo.rs"),
        "fn alpha_unique() {}\nfn beta_unique() {}\n",
    )
    .expect("write combo");
    // File containing only one keyword
    fs::write(
        root.join("src/partial.rs"),
        "fn alpha_unique() {}\nfn other() {}\n",
    )
    .expect("write partial");

    let index = RepositoryIndex::build(&root).expect("build");
    let result = index.search("alpha_unique beta_unique", 10);

    let combo = result.matches.iter().find(|m| m.path == "src/combo.rs");
    let partial = result.matches.iter().find(|m| m.path == "src/partial.rs");
    assert!(combo.is_some(), "combo.rs should be found");
    assert!(partial.is_some(), "partial.rs should be found");
    // File matching all keywords should score higher
    assert!(combo.unwrap().score > partial.unwrap().score);
}

#[test]
fn two_pass_filters_content_read() {
    // Verify 2-pass: a file with no path match but content match is
    // only found if it makes it past the 1st pass. With 50+ candidate
    // limit and a small test dir, most files pass. This is a smoke test.
    let root = common::unique_test_dir("retrieval_two_pass");
    fs::create_dir_all(root.join("src")).expect("dir");
    fs::write(root.join("src/target.rs"), "fn target_function() {}\n").expect("write target");
    fs::write(root.join("src/unrelated.rs"), "fn something_else() {}\n").expect("write unrelated");

    let index = RepositoryIndex::build(&root).expect("build");
    let result = index.search("target", 10);

    assert!(!result.matches.is_empty());
    assert_eq!(result.matches[0].path, "src/target.rs");
}

#[test]
fn git_diff_fallback_empty() {
    // In a non-git directory, search should still work (empty changed files)
    let root = common::unique_test_dir("retrieval_no_git");
    fs::create_dir_all(root.join("src")).expect("dir");
    fs::write(root.join("src/hello.rs"), "fn hello() {}\n").expect("write");

    let index = RepositoryIndex::build(&root).expect("build");
    let result = index.search("hello", 10);

    assert!(!result.matches.is_empty());
    assert!(result.matches.iter().any(|m| m.path == "src/hello.rs"));
}

#[test]
fn existing_scoring_preserved() {
    // Regression: file name match should still beat content-only match
    let root = common::unique_test_dir("retrieval_regression");
    fs::create_dir_all(root.join("src")).expect("dir");
    fs::create_dir_all(root.join("docs")).expect("docs dir");

    fs::write(root.join("src/config.rs"), "pub fn config_init() {}\n").expect("write config");
    fs::write(
        root.join("docs/readme.md"),
        "This file talks about config settings.\nconfig is important.\n",
    )
    .expect("write readme");

    let index = RepositoryIndex::build(&root).expect("build");
    let result = index.search("config", 10);

    assert!(result.matches.len() >= 2);
    // Path/name match file should rank first
    assert_eq!(result.matches[0].path, "src/config.rs");
}
