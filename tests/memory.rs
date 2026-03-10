use anvil::state::memory::MemoryStore;
use tempfile::tempdir;

#[test]
fn memory_add_appends_bullet_entry() {
    let dir = tempdir().unwrap();
    let store = MemoryStore::new(dir.path().join("ANVIL-MEMORY.md"));

    store.add_entry("Prefer concise status updates").unwrap();

    let loaded = store.load().unwrap();
    assert!(loaded.contains("# ANVIL Memory"));
    assert!(loaded.contains("- Prefer concise status updates"));
}

#[test]
fn memory_load_returns_default_header_when_missing() {
    let dir = tempdir().unwrap();
    let store = MemoryStore::new(dir.path().join("ANVIL-MEMORY.md"));

    let loaded = store.load().unwrap();

    assert_eq!(loaded, "# ANVIL Memory\n");
}
