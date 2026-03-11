use anvil::state::artifacts::ArtifactStore;
use tempfile::tempdir;

#[test]
fn artifact_store_writes_and_rotates_entries() {
    let dir = tempdir().unwrap();
    let store = ArtifactStore::new(dir.path(), 2);

    let a = store.write_text("tool-output", "first").unwrap();
    let b = store.write_text("tool-output", "second").unwrap();
    let c = store.write_text("tool-output", "third").unwrap();

    assert!(b.exists());
    assert!(c.exists());
    assert!(!a.exists());
}
