use anvil::policy::change_detection::{ChangeDetection, ChangeDetectionMethod};
use tempfile::tempdir;

#[test]
fn targeted_snapshot_diff_detects_changed_files() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, "one\n").unwrap();
    std::fs::write(&b, "two\n").unwrap();

    let detector = ChangeDetection::new(ChangeDetectionMethod::TargetedSnapshotDiff);
    let before = detector.snapshot(&[a.clone(), b.clone()]).unwrap();
    std::fs::write(&b, "changed\n").unwrap();
    let changed = detector.diff(&before, &[a, b]).unwrap();

    assert_eq!(changed.len(), 1);
    assert!(changed[0].path.ends_with("b.txt"));
}

#[test]
fn tool_reported_changes_are_preserved() {
    let detector = ChangeDetection::new(ChangeDetectionMethod::ToolReported);
    let changes = detector.from_reported(vec!["src/main.rs".into(), "README.md".into()]);

    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].path.to_string_lossy(), "src/main.rs");
}
