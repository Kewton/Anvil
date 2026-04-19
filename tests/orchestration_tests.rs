use anvil::agent::orchestration::{capture_repo_snapshot, verify_repo_progress};
use tempfile::tempdir;

#[test]
fn deterministic_verifier_reports_changed_file_categories() {
    let temp = tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::write(temp.path().join("package.json"), "{}").unwrap();
    let before = capture_repo_snapshot(temp.path());

    std::fs::write(temp.path().join("package.json"), "{\"test\":1}").unwrap();
    std::fs::write(
        temp.path().join("src/app.tsx"),
        "export default function App() {}",
    )
    .unwrap();
    std::fs::write(temp.path().join("src/app.test.ts"), "test('ok', () => {})").unwrap();

    let verification = verify_repo_progress(&before, temp.path());
    assert_eq!(verification.setup_files_changed, 1);
    assert_eq!(verification.implementation_files_changed, 1);
    assert_eq!(verification.test_files_changed, 1);
    assert!(verification.made_any_progress());
    assert_eq!(verification.changed_files.len(), 3);
}
