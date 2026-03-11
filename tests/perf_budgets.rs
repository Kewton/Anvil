use std::time::Duration;

use anvil::state::audit::AuditLog;
use anvil::state::summary::{LatencyBudget, SummaryController, SummaryPolicy};
use tempfile::tempdir;

#[test]
fn summary_and_subagent_latency_budgets_have_safe_defaults() {
    let budget = LatencyBudget::default();
    let controller = SummaryController::new(SummaryPolicy::default());

    assert!(controller.within_summary_budget(Duration::from_millis(300), budget));
    assert!(!controller.within_summary_budget(Duration::from_secs(2), budget));
    assert!(controller.within_subagent_budget(Duration::from_secs(2), budget));
    assert!(!controller.within_subagent_budget(Duration::from_secs(8), budget));
}

#[test]
fn audit_log_rotates_when_size_exceeds_budget() {
    let dir = tempdir().unwrap();
    let log = AuditLog::new(dir.path().join("audit.log.jsonl"));

    for i in 0..40 {
        log.append_raw_line(&format!(
            "{{\"n\":{i},\"payload\":\"{}\"}}",
            "x".repeat(100)
        ))
        .unwrap();
    }
    log.rotate_if_needed(512).unwrap();

    let rotated = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .any(|name| name.starts_with("audit.log.") && name.ends_with(".jsonl"));
    assert!(rotated);
}
