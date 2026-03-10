use anvil::state::session::Session;
use tempfile::tempdir;

#[test]
fn session_can_be_saved_and_loaded() {
    let dir = tempdir().unwrap();
    let session = Session::new(dir.path());
    let path = dir.path().join("session.json");

    session.save(&path).unwrap();
    let loaded = Session::load(&path).unwrap();

    assert_eq!(loaded.id, session.id);
    assert_eq!(loaded.root, session.root);
}
