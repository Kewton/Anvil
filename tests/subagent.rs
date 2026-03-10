use anvil::agent::subagent::{SubagentRequest, SubagentRunner};
use anvil::policy::permissions::PermissionCategory;
use anvil::state::audit::{AuditEventData, AuditLog};
use tempfile::tempdir;

#[test]
fn subagent_compresses_report_and_writes_artifact() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".anvil/state");
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("README.md"),
        "This project implements add and subtract helpers.\n",
    )
    .unwrap();

    let audit = AuditLog::new(state_dir.join("audit.log.jsonl"));
    let runner = SubagentRunner::new(dir.path(), &state_dir);
    let report = runner
        .run(
            "sess_main",
            &audit,
            SubagentRequest {
                task: "find add helper implementation".to_string(),
                granted_permissions: vec![PermissionCategory::SubagentRead],
            },
        )
        .unwrap();

    assert!(report.summary.len() <= 240);
    assert!(report.key_findings.len() <= 3);
    assert!(!report.referenced_files.is_empty());
    assert!(report.artifacts.iter().any(|p| p.exists()));
}

#[test]
fn subagent_logs_started_and_finished_events() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".anvil/state");
    std::fs::write(dir.path().join("notes.txt"), "hello\n").unwrap();

    let audit = AuditLog::new(state_dir.join("audit.log.jsonl"));
    let runner = SubagentRunner::new(dir.path(), &state_dir);
    runner
        .run(
            "sess_main",
            &audit,
            SubagentRequest {
                task: "inspect notes".to_string(),
                granted_permissions: vec![PermissionCategory::SubagentRead],
            },
        )
        .unwrap();

    let events = audit.load_all().unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event.data, AuditEventData::SubagentStarted { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event.data, AuditEventData::SubagentFinished { .. }))
    );
}

#[test]
fn subagent_write_permission_is_denied_by_default() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join(".anvil/state");
    let audit = AuditLog::new(state_dir.join("audit.log.jsonl"));
    let runner = SubagentRunner::new(dir.path(), &state_dir);

    let err = runner
        .run(
            "sess_main",
            &audit,
            SubagentRequest {
                task: "try to edit files".to_string(),
                granted_permissions: vec![PermissionCategory::SubagentWrite],
            },
        )
        .unwrap_err();

    assert!(format!("{err}").contains("not permitted"));
}
