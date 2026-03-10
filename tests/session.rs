use anvil::state::session::Session;
use tempfile::tempdir;

#[test]
fn session_id_has_expected_prefix() {
    let dir = tempdir().unwrap();
    let session = Session::new(dir.path());

    assert!(session.id.starts_with("sess_"));
    assert_eq!(session.root, dir.path());
}
