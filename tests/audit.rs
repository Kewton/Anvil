use anvil::policy::permissions::PermissionMode;
use anvil::state::audit::{
    AuditActor, AuditEvent, AuditEventData, AuditLog, AuditMetadata, AuditSource,
};
use tempfile::tempdir;

#[test]
fn audit_event_roundtrip_and_append() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("audit.log.jsonl");
    let log = AuditLog::new(&path);
    let event = AuditEvent {
        meta: AuditMetadata::new(
            "sess_test",
            AuditActor::MainAgent,
            AuditSource::OneShot,
            dir.path(),
        ),
        data: AuditEventData::SessionStarted {
            model: "qwen3.5:35b".to_string(),
            permission_mode: PermissionMode::Ask,
        },
    };
    log.append(&event).unwrap();
    let loaded = log.load_all().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0], event);
}
