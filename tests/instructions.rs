use std::fs;

use anvil::instructions::{find_anvil_md, load_instructions};
use tempfile::tempdir;

#[test]
fn finds_nearest_anvil_md() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("root");
    let nested = root.join("a/b");
    fs::create_dir_all(&nested).unwrap();
    fs::write(root.join("ANVIL.md"), "root").unwrap();
    fs::write(nested.join("ANVIL.md"), "nested").unwrap();

    let found = find_anvil_md(&nested).unwrap();
    assert_eq!(found, nested.join("ANVIL.md"));
}

#[test]
fn loads_memory_default() {
    let dir = tempdir().unwrap();
    let loaded = load_instructions(dir.path()).unwrap();
    assert!(loaded.memory_text.contains("ANVIL Memory"));
}
