use anvil::state::session::Session;
use tempfile::tempdir;

#[test]
fn session_can_be_saved_and_loaded() {
    let dir = tempdir().unwrap();
    let mut session = Session::new(dir.path());
    session.update_summary(Some("Rolling summary: remembered".to_string()), 14);
    let path = dir.path().join("session.json");

    session.save(&path).unwrap();
    let loaded = Session::load(&path).unwrap();

    assert_eq!(loaded.id, session.id);
    assert_eq!(loaded.root, session.root);
    assert_eq!(loaded.rolling_summary, session.rolling_summary);
    assert_eq!(loaded.summarized_events, 14);
}
