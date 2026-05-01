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
    assert_eq!(verification.other_files_changed, 0);
    assert_eq!(verification.deleted_files_changed, 0);
    assert!(verification.made_any_progress());
    assert_eq!(verification.changed_files.len(), 3);
    assert_eq!(verification.all_changed_files.len(), 3);
    assert_eq!(verification.total_changed_files(), 3);
}

#[test]
fn deleted_files_are_counted_with_suffix() {
    let temp = tempdir().unwrap();
    std::fs::write(temp.path().join("remove_me.rs"), "fn x() {}").unwrap();
    std::fs::write(temp.path().join("keep.rs"), "fn y() {}").unwrap();
    let before = capture_repo_snapshot(temp.path());

    std::fs::remove_file(temp.path().join("remove_me.rs")).unwrap();

    let verification = verify_repo_progress(&before, temp.path());
    assert_eq!(verification.deleted_files_changed, 1);
    assert!(
        verification
            .changed_files
            .iter()
            .any(|f| f.contains("(deleted)")),
        "expected (deleted) suffix, got: {:?}",
        verification.changed_files
    );
    assert!(
        verification
            .all_changed_files
            .iter()
            .any(|f| f.contains("(deleted)"))
    );
}

#[test]
fn total_changed_files_sums_all_categories() {
    let temp = tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::write(temp.path().join("delete_me.rs"), "fn a() {}").unwrap();
    let before = capture_repo_snapshot(temp.path());

    // create impl + test files, delete one
    std::fs::write(temp.path().join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(temp.path().join("src/main.test.ts"), "test('x', () => {})").unwrap();
    std::fs::remove_file(temp.path().join("delete_me.rs")).unwrap();

    let v = verify_repo_progress(&before, temp.path());
    assert_eq!(v.implementation_files_changed, 1);
    assert_eq!(v.test_files_changed, 1);
    assert_eq!(v.other_files_changed, 0);
    assert_eq!(v.deleted_files_changed, 1);
    assert_eq!(v.total_changed_files(), 3);
}

#[test]
fn markdown_docs_are_counted_as_other_changed_files() {
    let temp = tempdir().unwrap();
    std::fs::write(temp.path().join("README.md"), "# Before\n").unwrap();
    let before = capture_repo_snapshot(temp.path());

    std::fs::write(temp.path().join("README.md"), "# After\n").unwrap();

    let v = verify_repo_progress(&before, temp.path());
    assert_eq!(v.other_files_changed, 1);
    assert_eq!(v.total_changed_files(), 1);
    assert_eq!(v.changed_files, vec!["README.md".to_string()]);
    assert_eq!(v.all_changed_files, vec!["README.md".to_string()]);
}

#[test]
fn runtime_artifact_dirs_are_ignored() {
    let temp = tempdir().unwrap();
    std::fs::write(temp.path().join("calculator.py"), "def f(): return 1\n").unwrap();
    let before = capture_repo_snapshot(temp.path());

    std::fs::write(temp.path().join("calculator.py"), "def f(): return 2\n").unwrap();
    std::fs::create_dir_all(temp.path().join(".pytest_cache")).unwrap();
    std::fs::write(temp.path().join(".pytest_cache/README.md"), "cache\n").unwrap();
    std::fs::create_dir_all(temp.path().join("__pycache__")).unwrap();
    std::fs::write(temp.path().join("__pycache__/calculator.pyc"), "cache\n").unwrap();
    std::fs::create_dir_all(temp.path().join(".next/cache")).unwrap();
    std::fs::write(temp.path().join(".next/cache/build"), "cache\n").unwrap();

    let v = verify_repo_progress(&before, temp.path());
    assert_eq!(v.changed_files, vec!["calculator.py".to_string()]);
    assert_eq!(v.all_changed_files, vec!["calculator.py".to_string()]);
    assert_eq!(v.implementation_files_changed, 1);
    assert_eq!(v.total_changed_files(), 1);
}

#[test]
fn changed_files_capped_at_sixteen() {
    let temp = tempdir().unwrap();
    let before = capture_repo_snapshot(temp.path());

    for i in 0..20 {
        std::fs::write(
            temp.path().join(format!("file{i}.rs")),
            format!("fn f{i}() {{}}"),
        )
        .unwrap();
    }

    let v = verify_repo_progress(&before, temp.path());
    assert!(
        v.changed_files.len() <= 16,
        "cap exceeded: {}",
        v.changed_files.len()
    );
    assert_eq!(v.implementation_files_changed, 20);
    assert_eq!(v.all_changed_files.len(), 20);
    assert_eq!(v.total_changed_files(), 20);
}
