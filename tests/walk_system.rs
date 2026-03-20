use anvil::walk;
use std::fs;

#[test]
fn walk_should_skip_dir_returns_true_for_known_dirs() {
    assert!(walk::should_skip_dir(".git"));
    assert!(walk::should_skip_dir("target"));
    assert!(walk::should_skip_dir(".anvil"));
}

#[test]
fn walk_should_skip_dir_returns_false_for_normal_dirs() {
    assert!(!walk::should_skip_dir("src"));
    assert!(!walk::should_skip_dir("docs"));
    assert!(!walk::should_skip_dir("lib"));
    assert!(!walk::should_skip_dir("node_modules"));
}

#[test]
fn walk_is_binary_detects_binary_extensions() {
    use std::path::Path;
    for ext in walk::BINARY_EXTENSIONS {
        let p = Path::new("file").with_extension(ext);
        assert!(walk::is_binary(&p), "expected {ext} to be binary");
    }
}

#[test]
fn walk_is_binary_allows_text_files() {
    use std::path::Path;
    let text_exts = ["rs", "toml", "md", "txt", "json", "yaml", "py", "js", "ts"];
    for ext in &text_exts {
        let p = Path::new("file").with_extension(ext);
        assert!(!walk::is_binary(&p), "expected {ext} to NOT be binary");
    }
}

#[test]
fn walk_respects_skip_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Create normal file
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();

    // Create files inside skip dirs
    fs::create_dir_all(root.join(".git/objects")).unwrap();
    fs::write(root.join(".git/objects/abc"), "data").unwrap();
    fs::create_dir_all(root.join("target/debug")).unwrap();
    fs::write(root.join("target/debug/app"), "binary").unwrap();
    fs::create_dir_all(root.join(".anvil/state")).unwrap();
    fs::write(root.join(".anvil/state/cache.json"), "{}").unwrap();

    let files: Vec<_> = walk::walk(root).collect();
    let rel_paths: Vec<String> = files
        .iter()
        .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().to_string())
        .collect();

    assert!(
        rel_paths.contains(&"src/main.rs".to_string()),
        "should contain src/main.rs, got: {rel_paths:?}"
    );
    assert!(
        !rel_paths.iter().any(|p| p.starts_with(".git/")),
        "should not contain .git files"
    );
    assert!(
        !rel_paths.iter().any(|p| p.starts_with("target/")),
        "should not contain target files"
    );
    assert!(
        !rel_paths.iter().any(|p| p.starts_with(".anvil/")),
        "should not contain .anvil files"
    );
}

#[test]
fn walk_respects_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // Initialize a git repo so .gitignore is respected
    fs::create_dir_all(root.join(".git")).unwrap();

    // Create .gitignore
    fs::write(root.join(".gitignore"), "ignored_dir/\nignored.txt\n").unwrap();

    // Create files
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn lib() {}").unwrap();
    fs::create_dir_all(root.join("ignored_dir")).unwrap();
    fs::write(root.join("ignored_dir/secret.rs"), "secret").unwrap();
    fs::write(root.join("ignored.txt"), "ignored content").unwrap();
    fs::write(root.join("kept.txt"), "kept content").unwrap();

    let files: Vec<_> = walk::walk(root).collect();
    let rel_paths: Vec<String> = files
        .iter()
        .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().to_string())
        .collect();

    assert!(
        rel_paths.contains(&"src/lib.rs".to_string()),
        "should contain src/lib.rs"
    );
    assert!(
        rel_paths.contains(&"kept.txt".to_string()),
        "should contain kept.txt"
    );
    assert!(
        !rel_paths.iter().any(|p| p.contains("ignored_dir")),
        "should not contain ignored_dir files, got: {rel_paths:?}"
    );
    assert!(
        !rel_paths.contains(&"ignored.txt".to_string()),
        "should not contain ignored.txt, got: {rel_paths:?}"
    );
}
