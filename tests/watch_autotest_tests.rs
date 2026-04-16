use std::fs;

use anvil::testloop::auto_test::AutoTestRunner;
use anvil::watch::file_watcher::FileWatcher;
use tempfile::tempdir;

#[test]
fn file_watcher_detects_add_modify_remove() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("file.txt");
    fs::write(&path, "a").unwrap();
    let mut watcher = FileWatcher::new(dir.path().to_path_buf()).unwrap();

    fs::write(&path, "b").unwrap();
    let changes = watcher.poll_changes().unwrap();
    assert!(
        changes
            .iter()
            .any(|line| line.contains("modified file.txt"))
    );

    fs::remove_file(&path).unwrap();
    let changes = watcher.poll_changes().unwrap();
    assert!(changes.iter().any(|line| line.contains("removed file.txt")));
}

#[test]
fn auto_test_runner_executes_command() {
    let dir = tempdir().unwrap();
    let runner = AutoTestRunner::new(Some("printf 'auto-test-ok'".to_string()));
    let result = runner.run_if_enabled(dir.path()).unwrap().unwrap();
    assert_eq!(result.exit_code, 0);
    assert!(result.output.contains("auto-test-ok"));
}
