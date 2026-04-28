use anvil::session::compact::{
    COMPACT_SUMMARY_PREFIX, approximate_token_count, compact_messages,
    compact_messages_with_strategy,
};
use anvil::session::feedback::{FeedbackFrame, FeedbackKind};
use anvil::session::precaution::{
    AddPrecautionOutcome, Precaution, PrecautionSource, PrecautionStatus, RetiredReason, Severity,
};
use anvil::session::store::{
    ConversationMessage, SessionSnapshot, SessionStore, WorkingMemory, reconcile_resume_state,
};
use std::path::PathBuf;
use tempfile::tempdir;

/// Helper: build a minimal Precaution to feed into add_precaution. id/status
/// are overwritten by canonicalize_for_storage, but we still set them to
/// non-default values to assert the canonicalization actually happens.
fn sample_precaution(text: &str) -> Precaution {
    Precaution {
        id: "ignored-by-canonicalize".to_string(),
        source: PrecautionSource::Manual,
        severity: Severity::Medium,
        text: text.to_string(),
        applies_to: Vec::new(),
        status: PrecautionStatus::Active,
        retired_reason: None,
    }
}

#[test]
fn session_store_round_trip() {
    let dir = tempdir().unwrap();
    let session_id = "0199fe00-0000-7000-8000-000000000001";
    let workspace_key = "abc123";
    let session_dir = dir.path().join("sessions").join(session_id);
    std::fs::create_dir_all(&session_dir).unwrap();
    let store = SessionStore::new(dir.path(), session_id, workspace_key);
    let snapshot = SessionSnapshot {
        messages: vec![ConversationMessage::user("hello".into())],
        ..SessionSnapshot::default()
    };
    store.save(&snapshot).unwrap();
    let loaded = store.load_or_new(false).unwrap();
    assert_eq!(loaded.messages, snapshot.messages);
    assert_eq!(loaded.id, session_id);
    assert_eq!(loaded.workspace_key, workspace_key);
}

#[test]
fn session_snapshot_roundtrip_preserves_id_and_workspace_key() {
    let snapshot = SessionSnapshot {
        id: "0199fe00-0000-7000-8000-000000000002".to_string(),
        workspace_key: "deadbeef".to_string(),
        messages: vec![ConversationMessage::user("hi".into())],
        ..SessionSnapshot::default()
    };
    let json = serde_json::to_string(&snapshot).unwrap();
    let decoded: SessionSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.id, snapshot.id);
    assert_eq!(decoded.workspace_key, snapshot.workspace_key);
    assert_eq!(decoded.messages, snapshot.messages);
}

#[test]
fn session_snapshot_legacy_defaults_id_and_workspace_key() {
    let legacy =
        r#"{"mode_state":{"mode":"Act","active_plan_path":null},"messages":[],"checkpoints":[]}"#;
    let decoded: SessionSnapshot = serde_json::from_str(legacy).unwrap();
    assert_eq!(decoded.id, "");
    assert_eq!(decoded.workspace_key, "");
}

#[test]
fn session_store_fills_in_empty_id_on_load() {
    let dir = tempdir().unwrap();
    let session_id = "0199fe00-0000-7000-8000-000000000003";
    let workspace_key = "ws-key";
    let session_dir = dir.path().join("sessions").join(session_id);
    std::fs::create_dir_all(&session_dir).unwrap();
    let session_json = session_dir.join("session.json");
    std::fs::write(
        &session_json,
        r#"{"mode_state":{"mode":"Act","active_plan_path":null},"messages":[],"checkpoints":[]}"#,
    )
    .unwrap();

    let store = SessionStore::new(dir.path(), session_id, workspace_key);
    let loaded = store.load_or_new(false).unwrap();
    assert_eq!(loaded.id, session_id);
    assert_eq!(loaded.workspace_key, workspace_key);
}

#[test]
fn manual_precaution_survives_session_store_load_and_sanitize() {
    let dir = tempdir().unwrap();
    let session_id = "0199fe00-0000-7000-8000-000000000454";
    let workspace_key = "ws-key";
    let store = SessionStore::new(dir.path(), session_id, workspace_key);
    let mut snapshot = SessionSnapshot {
        id: session_id.to_string(),
        workspace_key: workspace_key.to_string(),
        ..SessionSnapshot::default()
    };
    snapshot
        .working_memory
        .add_precaution(sample_precaution("preserve manual precaution"), dir.path());
    let original_id = snapshot.working_memory.active_precautions[0].id.clone();

    store.save(&snapshot).unwrap();
    let mut loaded = store.load_or_new(false).unwrap();
    loaded
        .working_memory
        .sanitize_active_precautions_after_load(dir.path());

    assert_eq!(loaded.working_memory.active_precautions.len(), 1);
    let restored = &loaded.working_memory.active_precautions[0];
    assert_eq!(restored.id, original_id);
    assert_eq!(restored.source, PrecautionSource::Manual);
    assert_eq!(restored.status, PrecautionStatus::Active);
    assert_eq!(restored.text, "preserve manual precaution");
}

#[test]
fn compaction_keeps_recent_tail() {
    let mut messages = (0..40)
        .map(|index| ConversationMessage::user(format!("message {index}")))
        .collect::<Vec<_>>();
    assert!(compact_messages(&mut messages, 10));
    assert!(
        messages
            .iter()
            .any(|message| message.content.contains(COMPACT_SUMMARY_PREFIX))
    );
    assert!(messages.last().unwrap().content.contains("39"));
}

#[test]
fn compaction_can_use_custom_summary_strategy() {
    let mut messages = (0..20)
        .map(|index| ConversationMessage::user(format!("message {index}")))
        .collect::<Vec<_>>();
    let changed = compact_messages_with_strategy(&mut messages, 5, |_| {
        Ok("custom sidecar summary".to_string())
    })
    .unwrap();
    assert!(changed);
    assert!(
        messages
            .iter()
            .any(|message| message.content.contains("custom sidecar summary"))
    );
}

#[test]
fn approximate_token_count_is_positive() {
    let messages = vec![
        ConversationMessage::user("hello".into()),
        ConversationMessage::assistant("world".into(), Vec::new()),
    ];
    assert!(approximate_token_count(&messages) > 0);
}

#[test]
fn working_memory_formats_for_prompt() {
    let mut memory = WorkingMemory::default();
    memory.set_active_task(Some("Implement long-session memory".to_string()));
    memory.replace_constraints(vec!["Keep paths relative".to_string()]);
    memory.note_touched_file("src/main.rs".to_string());
    memory.note_error("Edit: target text not found".to_string());

    let rendered = memory.format_for_prompt().unwrap();
    assert!(rendered.contains("Active task: Implement long-session memory"));
    assert!(rendered.contains("Constraints:"));
    assert!(rendered.contains("src/main.rs"));
    assert!(rendered.contains("target text not found"));
}

#[test]
fn session_snapshot_roundtrip_preserves_working_memory() {
    let snapshot = SessionSnapshot {
        working_memory: WorkingMemory {
            active_task: Some("Do the thing".to_string()),
            constraints: vec!["Do not lose context".to_string()],
            touched_files: vec!["src/lib.rs".to_string()],
            unresolved_errors: vec!["Edit: target test not found".to_string()],
            active_precautions: vec![],
        },
        ..SessionSnapshot::default()
    };

    let json = serde_json::to_string(&snapshot).unwrap();
    let decoded: SessionSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.working_memory, snapshot.working_memory);
}

// --- Issue #450 AC7-AC9 (FeedbackFrame integration) ----------------------

/// Build a `FeedbackFrame` for tests by going through serde. This
/// deliberately avoids relying on `build_feedback_frame`, which is
/// `pub(crate)` — integration tests must observe the same shape that
/// any external persistence layer would see.
fn sample_feedback_frame() -> FeedbackFrame {
    let json = r#"{
        "exit_code": 101,
        "kind": "compile_error",
        "primary_error": "missing semicolon",
        "suspected_files": ["src/lib.rs"]
    }"#;
    serde_json::from_str(json).expect("frame deserializes")
}

/// AC7: a FeedbackFrame attached to `SessionSnapshot.last_feedback`
/// survives the save/load round-trip.
#[test]
fn last_feedback_persists_in_session_json() {
    let dir = tempdir().unwrap();
    let session_id = "0199fe00-0000-7000-8000-000000000010";
    let workspace_key = "ws-feedback";
    let session_dir = dir.path().join("sessions").join(session_id);
    std::fs::create_dir_all(&session_dir).unwrap();
    let store = SessionStore::new(dir.path(), session_id, workspace_key);

    let mut snapshot = SessionSnapshot::default();
    snapshot.record_feedback(sample_feedback_frame());

    store.save(&snapshot).unwrap();

    let loaded = store.load_or_new(false).unwrap();
    assert_eq!(loaded.last_feedback, snapshot.last_feedback);
    assert_eq!(
        loaded.last_feedback.unwrap().kind,
        FeedbackKind::CompileError
    );
}

/// AC8: after `--resume` (=`load_or_new(false)` then `reconcile_resume_state`)
/// the `last_feedback` is observable and unchanged.
#[test]
fn resume_loads_last_feedback() {
    let dir = tempdir().unwrap();
    let session_id = "0199fe00-0000-7000-8000-000000000011";
    let workspace_key = "ws-resume";
    let session_dir = dir.path().join("sessions").join(session_id);
    std::fs::create_dir_all(&session_dir).unwrap();
    let store = SessionStore::new(dir.path(), session_id, workspace_key);

    let mut snapshot = SessionSnapshot::default();
    snapshot.record_feedback(sample_feedback_frame());
    store.save(&snapshot).unwrap();

    let mut resumed = store.load_or_new(false).unwrap();
    assert!(resumed.last_feedback.is_some());

    let before = resumed.last_feedback.clone();
    reconcile_resume_state(&mut resumed, dir.path());
    assert_eq!(resumed.last_feedback, before);
}

/// AC9: legacy session.json (no `last_feedback` field at all) loads as
/// `last_feedback == None`. The serde `default` attribute is what makes
/// this work.
#[test]
fn legacy_session_without_last_feedback_loads_with_none() {
    // Hand-rolled JSON with no `last_feedback` key, as it would be
    // produced by an older binary.
    let legacy = r#"{
        "mode_state": {"mode": "Act", "active_plan_path": null},
        "messages": [],
        "checkpoints": [],
        "id": "0199fe00-0000-7000-8000-0000000000aa",
        "workspace_key": "legacy-ws"
    }"#;
    let decoded: SessionSnapshot = serde_json::from_str(legacy).unwrap();
    assert!(decoded.last_feedback.is_none());
    assert_eq!(decoded.id, "0199fe00-0000-7000-8000-0000000000aa");
}

/// `record_feedback` is the only public mutation entry point, and it
/// overwrites in place (last-write-wins semantics, design 5.5).
#[test]
fn record_feedback_overwrites_last_value() {
    let timeout: FeedbackFrame = serde_json::from_str(r#"{"kind":"timeout"}"#).unwrap();
    let compile: FeedbackFrame = serde_json::from_str(r#"{"kind":"compile_error"}"#).unwrap();
    let mut snap = SessionSnapshot::default();
    snap.record_feedback(timeout);
    snap.record_feedback(compile);
    assert_eq!(snap.last_feedback.unwrap().kind, FeedbackKind::CompileError);
}

/// `reconcile_resume_state` does not touch `last_feedback`, even when
/// it has to clear `active_root` or downgrade the mode.
#[test]
fn reconcile_resume_state_preserves_last_feedback() {
    let frame = sample_feedback_frame();
    let mut snap = SessionSnapshot {
        active_root: Some(PathBuf::from("/nonexistent/path/for/test")),
        last_feedback: Some(frame.clone()),
        ..SessionSnapshot::default()
    };
    reconcile_resume_state(&mut snap, &PathBuf::from("/tmp"));
    // active_root should be cleared (broken).
    assert!(snap.active_root.is_none());
    // last_feedback must survive.
    assert_eq!(snap.last_feedback, Some(frame));
}

// --- Issue #455 / Task 4.1: NoToolCall session.json compat ---------------

/// AC9 extension: legacy session.json saved by an older binary will not
/// contain `last_feedback.kind == "no_tool_call"` because the variant did
/// not exist. The new binary must still load such files cleanly. The
/// `last_feedback` field is absent here so it stays `None`.
#[test]
fn legacy_session_loads_without_no_tool_call_kind() {
    let legacy = r#"{
        "mode_state": {"mode": "Act", "active_plan_path": null},
        "messages": [],
        "checkpoints": [],
        "id": "0199fe00-0000-7000-8000-000000000455",
        "workspace_key": "legacy-no-no-tool-call"
    }"#;
    let decoded: SessionSnapshot = serde_json::from_str(legacy).unwrap();
    assert!(decoded.last_feedback.is_none());
}

/// Issue #455 / D1: a `last_feedback` carrying `NoToolCall` must round-trip
/// through serde / SessionStore without losing the variant tag.
#[test]
fn no_tool_call_session_round_trips() {
    let dir = tempdir().unwrap();
    let session_id = "0199fe00-0000-7000-8000-000000000456";
    let workspace_key = "ws-no-tool-call";
    let session_dir = dir.path().join("sessions").join(session_id);
    std::fs::create_dir_all(&session_dir).unwrap();
    let store = SessionStore::new(dir.path(), session_id, workspace_key);

    // Build a session with last_feedback.kind == NoToolCall.
    let frame: FeedbackFrame = serde_json::from_str(
        r#"{"kind":"no_tool_call","primary_error":"no_tool_retries_exhausted"}"#,
    )
    .unwrap();
    let mut snapshot = SessionSnapshot::default();
    snapshot.record_feedback(frame.clone());

    store.save(&snapshot).unwrap();
    let loaded = store.load_or_new(false).unwrap();
    assert_eq!(loaded.last_feedback, Some(frame));
    assert_eq!(loaded.last_feedback.unwrap().kind, FeedbackKind::NoToolCall);
}

/// Issue #455 / DR3-004: an unknown future variant must continue to
/// deserialize as `UnknownFailure` (the `#[serde(other)]` fallback). This
/// guards against forward-compat regression after adding `NoToolCall`.
#[test]
fn unknown_kind_session_falls_back_to_unknown_failure() {
    let json = r#"{
        "mode_state": {"mode": "Act", "active_plan_path": null},
        "messages": [],
        "checkpoints": [],
        "id": "0199fe00-0000-7000-8000-000000000457",
        "workspace_key": "ws-unknown",
        "last_feedback": {"kind": "future_unseen_kind"}
    }"#;
    let decoded: SessionSnapshot = serde_json::from_str(json).unwrap();
    let kind = decoded.last_feedback.expect("last_feedback present").kind;
    assert_eq!(kind, FeedbackKind::UnknownFailure);
}

/// Issue #455 / DR1-004: NoToolCall must survive message compaction. The
/// compact pass only touches the messages Vec, but we explicitly pin this
/// invariant so a future refactor cannot accidentally clear last_feedback.
#[test]
fn no_tool_call_preserved_after_compact() {
    use anvil::session::compact::compact_messages;
    let frame: FeedbackFrame = serde_json::from_str(r#"{"kind":"no_tool_call"}"#).unwrap();
    let mut snapshot = SessionSnapshot::default();
    snapshot.record_feedback(frame.clone());
    snapshot.messages = (0..40)
        .map(|index| ConversationMessage::user(format!("msg {index}")))
        .collect();

    let changed = compact_messages(&mut snapshot.messages, 10);
    assert!(changed, "compact should run");
    // last_feedback must still be NoToolCall.
    assert_eq!(snapshot.last_feedback, Some(frame));
}

// --- Issue #451 (active_precautions) -----------------------------------------

/// TDD step 1: Precaution serde roundtrip + Default values.
#[test]
fn precaution_serde_roundtrip_default_and_full() {
    // Defaults documented in the design policy (DR1-005, DR1-008, DR1-011).
    assert_eq!(Severity::default(), Severity::Medium);
    assert_eq!(PrecautionStatus::default(), PrecautionStatus::Active);
    assert_eq!(PrecautionSource::default(), PrecautionSource::Manual);
    assert_eq!(RetiredReason::default(), RetiredReason::UserRetired);

    let p = Precaution {
        id: "deadbeef".to_string(),
        source: PrecautionSource::BuildFailure,
        severity: Severity::High,
        text: "Avoid using sed for in-place edits".to_string(),
        applies_to: vec![PathBuf::from("src/lib.rs")],
        status: PrecautionStatus::Active,
        retired_reason: None,
    };
    let json = serde_json::to_string(&p).unwrap();
    let decoded: Precaution = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, p);
}

/// AC3: same canonical key (text + source + applies_to) should be ignored on
/// the second add when the previous one is still Active.
#[test]
fn add_precaution_returns_added_then_duplicate_ignored() {
    let dir = tempdir().unwrap();
    let mut wm = WorkingMemory::default();
    let outcome1 = wm.add_precaution(sample_precaution("avoid sed"), dir.path());
    assert_eq!(outcome1, AddPrecautionOutcome::Added);
    let outcome2 = wm.add_precaution(sample_precaution("avoid sed"), dir.path());
    assert_eq!(outcome2, AddPrecautionOutcome::DuplicateIgnored);
    assert_eq!(wm.active_precautions.len(), 1);
}

/// Design judgment #6: text over MAX_PRECAUTION_TEXT is always stored, but
/// add_precaution returns Truncated to flag the trim.
#[test]
fn add_precaution_returns_truncated_when_over_text_limit() {
    let dir = tempdir().unwrap();
    let mut wm = WorkingMemory::default();
    let big = "a".repeat(WorkingMemory::MAX_PRECAUTION_TEXT + 50);
    let outcome = wm.add_precaution(sample_precaution(&big), dir.path());
    assert_eq!(outcome, AddPrecautionOutcome::Truncated);
    let stored = &wm.active_precautions[0];
    assert!(stored.text.ends_with("..."));
    // truncate_entry returns max_chars + 3 chars (the "..." marker).
    assert_eq!(
        stored.text.chars().count(),
        WorkingMemory::MAX_PRECAUTION_TEXT + 3
    );
}

/// FIFO eviction: the oldest Active is moved to Retired(CapacityEvicted) when
/// MAX_ACTIVE_PRECAUTIONS is reached.
#[test]
fn add_precaution_evicts_oldest_active_at_max_capacity() {
    let dir = tempdir().unwrap();
    let mut wm = WorkingMemory::default();
    for i in 0..WorkingMemory::MAX_ACTIVE_PRECAUTIONS {
        let outcome = wm.add_precaution(sample_precaution(&format!("p{i}")), dir.path());
        assert_eq!(outcome, AddPrecautionOutcome::Added);
    }
    let active_before = wm
        .active_precautions
        .iter()
        .filter(|p| p.status == PrecautionStatus::Active)
        .count();
    assert_eq!(active_before, WorkingMemory::MAX_ACTIVE_PRECAUTIONS);

    // Adding one more should evict the oldest Active to Retired(CapacityEvicted).
    let outcome = wm.add_precaution(sample_precaution("overflow"), dir.path());
    assert_eq!(outcome, AddPrecautionOutcome::Added);

    let active_after = wm
        .active_precautions
        .iter()
        .filter(|p| p.status == PrecautionStatus::Active)
        .count();
    assert_eq!(active_after, WorkingMemory::MAX_ACTIVE_PRECAUTIONS);

    // The evicted entry should be Retired with CapacityEvicted reason.
    let evicted = wm
        .active_precautions
        .iter()
        .find(|p| p.text == "p0")
        .expect("oldest precaution still in vec");
    assert_eq!(evicted.status, PrecautionStatus::Retired);
    assert_eq!(evicted.retired_reason, Some(RetiredReason::CapacityEvicted));
}

/// AC10: re-adding the same canonical key after CapacityEvicted is allowed
/// (status flips back to Active via a new entry).
#[test]
fn add_precaution_readds_after_capacity_eviction() {
    let dir = tempdir().unwrap();
    let mut wm = WorkingMemory::default();
    for i in 0..WorkingMemory::MAX_ACTIVE_PRECAUTIONS {
        wm.add_precaution(sample_precaution(&format!("p{i}")), dir.path());
    }
    // Push one more so that p0 is evicted to Retired(CapacityEvicted).
    wm.add_precaution(sample_precaution("overflow"), dir.path());

    // Re-add p0 — duplicate detection must NOT block this because the prior
    // p0 entry was Retired(CapacityEvicted), not UserRetired.
    let outcome = wm.add_precaution(sample_precaution("p0"), dir.path());
    assert_eq!(outcome, AddPrecautionOutcome::Added);

    let active_with_p0_text = wm
        .active_precautions
        .iter()
        .filter(|p| p.text == "p0" && p.status == PrecautionStatus::Active)
        .count();
    assert_eq!(active_with_p0_text, 1);
}

/// AC6: resolve_precaution returns false when the id does not exist.
#[test]
fn resolve_precaution_returns_false_for_unknown_id() {
    let dir = tempdir().unwrap();
    let mut wm = WorkingMemory::default();
    wm.add_precaution(sample_precaution("known"), dir.path());
    assert!(!wm.resolve_precaution("definitely-not-an-id"));
    // existing id is still Active.
    let id = wm.active_precautions[0].id.clone();
    assert!(wm.resolve_precaution(&id));
    assert_eq!(wm.active_precautions[0].status, PrecautionStatus::Resolved);
}

/// AC6: retire is terminal — UserRetired blocks re-Activation of the same key.
#[test]
fn retire_precaution_is_terminal_state() {
    let dir = tempdir().unwrap();
    let mut wm = WorkingMemory::default();
    wm.add_precaution(sample_precaution("retire-me"), dir.path());
    let id = wm.active_precautions[0].id.clone();
    assert!(wm.retire_precaution(&id));
    assert_eq!(wm.active_precautions[0].status, PrecautionStatus::Retired);
    assert_eq!(
        wm.active_precautions[0].retired_reason,
        Some(RetiredReason::UserRetired)
    );

    // Re-adding the same canonical key must be blocked (DuplicateIgnored).
    let outcome = wm.add_precaution(sample_precaution("retire-me"), dir.path());
    assert_eq!(outcome, AddPrecautionOutcome::DuplicateIgnored);
}

/// AC4: format_for_prompt includes Active precautions with severity label.
#[test]
fn working_memory_formats_for_prompt_includes_active_precautions() {
    let dir = tempdir().unwrap();
    let mut wm = WorkingMemory::default();
    let mut p = sample_precaution("Avoid using sed for in-place edits");
    p.severity = Severity::High;
    wm.add_precaution(p, dir.path());

    let rendered = wm.format_for_prompt().expect("active precaution renders");
    assert!(rendered.contains("Active Precautions:"));
    assert!(rendered.contains("- [high] Avoid using sed for in-place edits"));
}

/// AC5: format_for_prompt does not list Resolved or Retired precautions.
#[test]
fn working_memory_format_for_prompt_excludes_resolved_and_retired() {
    let dir = tempdir().unwrap();
    let mut wm = WorkingMemory::default();
    wm.add_precaution(sample_precaution("active-one"), dir.path());
    wm.add_precaution(sample_precaution("resolved-one"), dir.path());
    wm.add_precaution(sample_precaution("retired-one"), dir.path());

    let id_resolved = wm.active_precautions[1].id.clone();
    let id_retired = wm.active_precautions[2].id.clone();
    assert!(wm.resolve_precaution(&id_resolved));
    assert!(wm.retire_precaution(&id_retired));

    let rendered = wm.format_for_prompt().expect("active precaution renders");
    assert!(rendered.contains("active-one"));
    assert!(!rendered.contains("resolved-one"));
    assert!(!rendered.contains("retired-one"));
}

/// DR3-001: total Vec is bounded at MAX_PRECAUTIONS_TOTAL by pruning history.
#[test]
fn add_precaution_prunes_history_at_total_capacity() {
    let dir = tempdir().unwrap();
    let mut wm = WorkingMemory::default();

    // Push more than MAX_PRECAUTIONS_TOTAL adds. Each new push past
    // MAX_ACTIVE_PRECAUTIONS evicts an Active to Retired(CapacityEvicted).
    // The total Vec must never exceed MAX_PRECAUTIONS_TOTAL.
    for i in 0..(WorkingMemory::MAX_PRECAUTIONS_TOTAL + 20) {
        wm.add_precaution(sample_precaution(&format!("p{i}")), dir.path());
    }
    assert!(
        wm.active_precautions.len() <= WorkingMemory::MAX_PRECAUTIONS_TOTAL,
        "vec length exceeded total cap: {}",
        wm.active_precautions.len()
    );

    let active_count = wm
        .active_precautions
        .iter()
        .filter(|p| p.status == PrecautionStatus::Active)
        .count();
    assert_eq!(
        active_count,
        WorkingMemory::MAX_ACTIVE_PRECAUTIONS,
        "active count must remain at MAX_ACTIVE_PRECAUTIONS"
    );
}

/// AC8: legacy session.json without active_precautions field loads as empty Vec.
#[test]
fn legacy_session_without_active_precautions_loads_with_empty_vec() {
    let legacy = r#"{
        "mode_state": {"mode": "Act", "active_plan_path": null},
        "messages": [],
        "checkpoints": [],
        "id": "0199fe00-0000-7000-8000-0000000000bb",
        "workspace_key": "legacy-ws",
        "working_memory": {
            "active_task": "compute the answer",
            "constraints": [],
            "touched_files": [],
            "unresolved_errors": []
        }
    }"#;
    let decoded: SessionSnapshot = serde_json::from_str(legacy).unwrap();
    assert_eq!(decoded.working_memory.active_precautions.len(), 0);
    assert_eq!(
        decoded.working_memory.active_task.as_deref(),
        Some("compute the answer")
    );
}

/// AC13: unknown PrecautionSource string deserializes to Unknown variant
/// rather than failing or panicking.
#[test]
fn session_with_unknown_precaution_source_loads_as_unknown_variant() {
    let json = r#"{
        "id": "abc",
        "source": "future_unseen_source",
        "severity": "high",
        "text": "look out!",
        "status": "active"
    }"#;
    let decoded: Precaution = serde_json::from_str(json).unwrap();
    assert_eq!(decoded.source, PrecautionSource::Unknown);
}

/// DR3-005: id is stable through serde + sanitize roundtrip.
#[test]
fn active_precaution_id_stable_after_session_roundtrip() {
    let dir = tempdir().unwrap();
    let mut wm = WorkingMemory::default();
    wm.add_precaution(sample_precaution("stable-text"), dir.path());
    let id_before = wm.active_precautions[0].id.clone();

    let json = serde_json::to_string(&wm).unwrap();
    let mut decoded: WorkingMemory = serde_json::from_str(&json).unwrap();
    decoded.sanitize_active_precautions_after_load(dir.path());

    assert_eq!(decoded.active_precautions[0].id, id_before);
}

/// DR4-001/002/003: load-side sanitizer re-applies mask + truncate + path
/// normalization + Unknown-status -> Retired(Unknown), even from a manually
/// edited session.json.
#[test]
fn session_load_sanitizes_active_precautions_from_untrusted_json() {
    let dir = tempdir().unwrap();
    // Build a tampered WorkingMemory: text contains a fake AWS key; status is
    // an unknown string; id is a lie; applies_to has a `..` traversal entry.
    let tampered_text = "leak token: AKIAIOSFODNN7EXAMPLE here";
    let payload = format!(
        r#"{{
            "active_task": null,
            "active_precautions": [
                {{
                    "id": "totally-fake-id",
                    "source": "manual",
                    "severity": "medium",
                    "text": "{}",
                    "applies_to": ["../escape/secret.rs"],
                    "status": "future_unseen_status"
                }}
            ]
        }}"#,
        tampered_text
    );

    let mut wm: WorkingMemory = serde_json::from_str(&payload).unwrap();
    // Status deserialized as Unknown.
    assert_eq!(wm.active_precautions[0].status, PrecautionStatus::Unknown);

    wm.sanitize_active_precautions_after_load(dir.path());
    let p = &wm.active_precautions[0];

    // Token must have been masked.
    assert!(
        !p.text.contains("AKIAIOSFODNN7EXAMPLE"),
        "raw token survived sanitize: {}",
        p.text
    );
    // Path traversal entry dropped.
    assert!(p.applies_to.is_empty());
    // id has been recomputed (full SHA-256 hex = 64 chars).
    assert_ne!(p.id, "totally-fake-id");
    assert_eq!(p.id.len(), 64);
    assert!(p.id.chars().all(|c| c.is_ascii_hexdigit()));
    // Unknown status downgraded to Retired(Unknown).
    assert_eq!(p.status, PrecautionStatus::Retired);
    assert_eq!(p.retired_reason, Some(RetiredReason::Unknown));
}

/// DR4-004: a non-UTF-8 component in applies_to drops the whole path entry,
/// not just the offending component (no path collapse).
#[test]
fn canonicalize_applies_to_drops_non_utf8_path_without_path_collapse() {
    let dir = tempdir().unwrap();
    // Construct a path with a non-UTF-8 byte in one component.
    let bad_path = {
        #[cfg(unix)]
        {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;
            // bytes[1] is 0xFF -> not valid UTF-8.
            let bytes: &[u8] = b"src/\xFFbad.rs";
            PathBuf::from(OsStr::from_bytes(bytes))
        }
        #[cfg(not(unix))]
        {
            // On non-Unix targets we cannot easily craft a non-UTF-8 component.
            // Skip the case by using a benign path; the assertion still holds
            // because ParentDir / non-existent paths are dropped.
            PathBuf::from("ok.rs")
        }
    };
    let mut wm = WorkingMemory::default();
    let p = Precaution {
        id: String::new(),
        source: PrecautionSource::Manual,
        severity: Severity::Medium,
        text: "non-utf8 path test".to_string(),
        applies_to: vec![bad_path.clone()],
        status: PrecautionStatus::Active,
        retired_reason: None,
    };
    wm.add_precaution(p, dir.path());
    let stored = &wm.active_precautions[0];
    #[cfg(unix)]
    {
        // Non-UTF-8 path must not have been silently collapsed into a partial
        // path; it must have been dropped entirely.
        assert!(stored.applies_to.is_empty());
    }
    #[cfg(not(unix))]
    {
        // On non-Unix the test path is benign — just confirm it survives.
        let _ = stored;
    }
}

// --- Issue #451 iter-2 hardening (CB-001..CB-006) ----------------------------

/// CB-003: unknown Severity string in session.json must deserialize to
/// `Severity::Unknown` so the load does not fail. Without this fallback an old
/// session.json containing a future severity value would error out and resume
/// would be impossible (Opus AC13).
#[test]
fn severity_unknown_variant_falls_back_in_serde() {
    let json = r#"{
        "id": "abc",
        "source": "manual",
        "severity": "future_severity",
        "text": "with future severity",
        "status": "active"
    }"#;
    let decoded: Precaution = serde_json::from_str(json).unwrap();
    assert_eq!(decoded.severity, Severity::Unknown);
}

/// CB-003: load-side sanitizer rewrites `Severity::Unknown` to `Severity::Medium`
/// so the prompt format still uses a known label.
#[test]
fn canonicalize_normalizes_severity_unknown_to_medium() {
    let dir = tempdir().unwrap();
    let json = r#"{
        "active_precautions": [
            {
                "id": "fake",
                "source": "manual",
                "severity": "future_severity",
                "text": "must end up medium",
                "applies_to": [],
                "status": "active"
            }
        ]
    }"#;
    let mut wm: WorkingMemory = serde_json::from_str(json).unwrap();
    assert_eq!(wm.active_precautions[0].severity, Severity::Unknown);
    wm.sanitize_active_precautions_after_load(dir.path());
    assert_eq!(wm.active_precautions[0].severity, Severity::Medium);
}

/// CB-001: a tampered session.json with 50 same-id Active entries must be
/// reduced to a single Active (duplicate detection runs in the sanitizer too)
/// and `MAX_ACTIVE_PRECAUTIONS` is preserved.
#[test]
fn sanitize_caps_active_at_max_active_precautions() {
    let dir = tempdir().unwrap();
    // Build 50 Active entries with distinct text so they have distinct ids.
    let mut entries = Vec::new();
    for i in 0..50 {
        entries.push(Precaution {
            id: format!("loaded-id-{i}"),
            source: PrecautionSource::Manual,
            severity: Severity::Medium,
            text: format!("tampered-{i}"),
            applies_to: Vec::new(),
            status: PrecautionStatus::Active,
            retired_reason: None,
        });
    }
    let mut wm = WorkingMemory {
        active_precautions: entries,
        ..WorkingMemory::default()
    };
    wm.sanitize_active_precautions_after_load(dir.path());
    let active = wm
        .active_precautions
        .iter()
        .filter(|p| p.status == PrecautionStatus::Active)
        .count();
    assert!(
        active <= WorkingMemory::MAX_ACTIVE_PRECAUTIONS,
        "active count {active} exceeded MAX_ACTIVE_PRECAUTIONS"
    );
    assert!(wm.active_precautions.len() <= WorkingMemory::MAX_PRECAUTIONS_TOTAL);
}

/// CB-001: a tampered session.json with > MAX_PRECAUTIONS_TOTAL entries (all
/// Retired with Unknown / None reason — not currently a victim category) must
/// still be capped at the total limit by the final-fail-safe truncate.
#[test]
fn sanitize_caps_total_at_max_precautions_total() {
    let dir = tempdir().unwrap();
    let total = WorkingMemory::MAX_PRECAUTIONS_TOTAL + 30;
    let mut entries = Vec::new();
    for i in 0..total {
        entries.push(Precaution {
            id: format!("retired-{i}"),
            source: PrecautionSource::Manual,
            severity: Severity::Medium,
            text: format!("retired-text-{i}"),
            applies_to: Vec::new(),
            status: PrecautionStatus::Retired,
            // No retired_reason — exercise the previously-unreachable victim
            // path that would have allowed total > MAX_PRECAUTIONS_TOTAL.
            retired_reason: None,
        });
    }
    let mut wm = WorkingMemory {
        active_precautions: entries,
        ..WorkingMemory::default()
    };
    wm.sanitize_active_precautions_after_load(dir.path());
    assert!(
        wm.active_precautions.len() <= WorkingMemory::MAX_PRECAUTIONS_TOTAL,
        "total length {} exceeded MAX_PRECAUTIONS_TOTAL",
        wm.active_precautions.len()
    );
}

/// CB-001: duplicate Active entries with the same canonical id must collapse
/// to one entry. (After canonicalization, identical text+source+applies_to
/// produce the same id; the sanitizer must drop the dup, not preserve it.)
#[test]
fn sanitize_drops_duplicate_ids_in_loaded_active() {
    let dir = tempdir().unwrap();
    let entries = (0..5)
        .map(|_| Precaution {
            id: "ignored".to_string(),
            source: PrecautionSource::Manual,
            severity: Severity::Medium,
            text: "same canonical text".to_string(),
            applies_to: Vec::new(),
            status: PrecautionStatus::Active,
            retired_reason: None,
        })
        .collect::<Vec<_>>();
    let mut wm = WorkingMemory {
        active_precautions: entries,
        ..WorkingMemory::default()
    };
    wm.sanitize_active_precautions_after_load(dir.path());
    let active: Vec<&Precaution> = wm
        .active_precautions
        .iter()
        .filter(|p| p.status == PrecautionStatus::Active)
        .collect();
    assert_eq!(
        active.len(),
        1,
        "duplicate Active ids should collapse to one entry"
    );
}

/// CB-002: huge applies_to must be capped at `MAX_PRECAUTION_APPLIES_TO` to
/// prevent quadratic-ish work in canonicalize/sort/dedup and excess fs
/// canonicalize syscalls. We feed many distinct relative paths and assert the
/// stored `applies_to` does not exceed the cap.
#[test]
fn canonicalize_applies_to_caps_input_at_max_applies_to() {
    let dir = tempdir().unwrap();
    // Touch enough files inside workspace so canonicalize succeeds.
    let cap = WorkingMemory::MAX_PRECAUTION_APPLIES_TO;
    let mut paths = Vec::new();
    for i in 0..(cap + 50) {
        let rel = PathBuf::from(format!("file_{i}.rs"));
        std::fs::write(dir.path().join(&rel), "").unwrap();
        paths.push(rel);
    }
    let mut wm = WorkingMemory::default();
    let p = Precaution {
        id: String::new(),
        source: PrecautionSource::Manual,
        severity: Severity::Medium,
        text: "many paths".to_string(),
        applies_to: paths,
        status: PrecautionStatus::Active,
        retired_reason: None,
    };
    wm.add_precaution(p, dir.path());
    let stored = &wm.active_precautions[0];
    assert!(
        stored.applies_to.len() <= cap,
        "applies_to len {} exceeded cap {cap}",
        stored.applies_to.len()
    );
}

/// CB-004: an absolute path that points outside the workspace and cannot be
/// canonicalized into the workspace must be dropped entirely (no `file_name`
/// fallback that would leak the basename).
#[test]
fn canonicalize_applies_to_drops_absolute_outside_workspace() {
    let dir = tempdir().unwrap();
    // /etc/passwd_does_not_exist_likely is outside the workspace, even on
    // platforms where it could exist; if it canonicalizes to outside `dir`,
    // strip_prefix will fail and the path must be dropped.
    let outside = PathBuf::from("/this/absolute/path/should/not/exist/leak.rs");
    let mut wm = WorkingMemory::default();
    let p = Precaution {
        id: String::new(),
        source: PrecautionSource::Manual,
        severity: Severity::Medium,
        text: "outside path test".to_string(),
        applies_to: vec![outside],
        status: PrecautionStatus::Active,
        retired_reason: None,
    };
    wm.add_precaution(p, dir.path());
    let stored = &wm.active_precautions[0];
    assert!(
        stored.applies_to.is_empty(),
        "absolute outside path leaked basename: {:?}",
        stored.applies_to
    );
}

/// CB-004: a relative path that does not exist (broken / pre-create) must NOT
/// fall back to `file_name()` if doing so would leak just a basename. We only
/// keep paths that round-trip through workspace-prefixed canonicalize.
#[test]
fn canonicalize_applies_to_drops_broken_symlink_basename() {
    let dir = tempdir().unwrap();
    // Compose path with multiple components that does not exist in the
    // workspace. The previous behaviour would have stored "leak.rs" via
    // `Path::file_name()`. We require it to be dropped now.
    let nonexistent = PathBuf::from("nested/dir/leak.rs");
    let mut wm = WorkingMemory::default();
    let p = Precaution {
        id: String::new(),
        source: PrecautionSource::Manual,
        severity: Severity::Medium,
        text: "broken path test".to_string(),
        applies_to: vec![nonexistent],
        status: PrecautionStatus::Active,
        retired_reason: None,
    };
    wm.add_precaution(p, dir.path());
    let stored = &wm.active_precautions[0];
    // Must not have been collapsed to bare "leak.rs".
    assert!(
        !stored
            .applies_to
            .iter()
            .any(|p| p == &PathBuf::from("leak.rs")),
        "broken-path basename leaked: {:?}",
        stored.applies_to
    );
}

/// CB-006: precaution.text must have control chars stripped and newlines
/// converted to spaces so that prompt sections cannot be spoofed.
#[test]
fn add_precaution_strips_control_chars_and_newlines_from_text() {
    let dir = tempdir().unwrap();
    let mut wm = WorkingMemory::default();
    // Embed \n + ANSI escape (\x1b) + bell (\x07).
    let text = "line one\nUnresolved errors:\n- spoofed\x1b[31mred\x07";
    let p = sample_precaution(text);
    wm.add_precaution(p, dir.path());
    let stored = &wm.active_precautions[0];
    assert!(!stored.text.contains('\n'), "newline survived sanitize");
    assert!(!stored.text.contains('\r'), "CR survived sanitize");
    assert!(
        !stored.text.contains('\x1b'),
        "ANSI escape survived sanitize"
    );
    assert!(!stored.text.contains('\x07'), "bell survived sanitize");
}

/// CB-006: a multi-line text in a precaution must not be able to inject
/// fake additional sections into the prompt output.
#[test]
fn format_for_prompt_does_not_break_section_with_newline_in_text() {
    let dir = tempdir().unwrap();
    let mut wm = WorkingMemory::default();
    let mut p = sample_precaution("first line\nUnresolved errors:\n- spoofed");
    p.severity = Severity::High;
    wm.add_precaution(p, dir.path());
    let rendered = wm.format_for_prompt().expect("active precaution renders");
    // The active-precaution bullet must be a single line so the section
    // structure cannot be broken by the text payload.
    let active_bullet_line = rendered
        .lines()
        .find(|l| l.starts_with("- [high]"))
        .expect("active-precaution bullet present");
    assert!(
        !active_bullet_line.contains('\n'),
        "bullet line should be single-line"
    );
    // The text payload should be on the bullet line itself, not on its own
    // standalone line that could masquerade as a section header.
    let lines: Vec<&str> = rendered.lines().collect();
    // Specifically: no line should be exactly "Unresolved errors:" — wm has
    // none, but the forged payload tries to inject one as a header. Lines
    // beginning with `- ` (bullet continuation) are also forbidden as forgery
    // of a Resolved/Retired bullet. Bullets only come from `- [<sev>] `.
    for l in &lines {
        assert_ne!(
            l.trim(),
            "Unresolved errors:",
            "text payload forged a section header line: {rendered}"
        );
        // Forged "- spoofed" stand-alone bullet would slip past — it must not
        // be on its own line.
        assert_ne!(
            l.trim(),
            "- spoofed",
            "text payload forged a stand-alone bullet line: {rendered}"
        );
    }
}

// ============================================================================
// Issue #456: AnvilScore persistence / lossy / discovery / oversized tests
// ============================================================================

mod anvil_score_tests {
    use super::*;
    use anvil::session::anvil_score::{
        AnvilScore, MAX_ANVIL_SCORE_COUNT, MAX_ANVIL_SCORE_RAW_BYTES,
    };
    use anvil::session::discovery::{MAX_SESSION_JSON_BYTES, iter_session_dirs};
    use std::fs;

    fn write_session_dir(state_root: &std::path::Path, id: &str) -> std::path::PathBuf {
        let session_dir = state_root.join("sessions").join(id);
        fs::create_dir_all(&session_dir).unwrap();
        session_dir
    }

    fn fixture_score() -> AnvilScore {
        AnvilScore {
            build_passed: Some(true),
            tests_passed: Some(false),
            compile_errors_delta: Some(-1),
            test_failures_delta: Some(2),
            compile_error_count: Some(0),
            test_failure_count: Some(2),
            implementation_files_changed: Some(3),
            test_files_changed: Some(1),
            setup_files_changed: Some(0),
            unsafe_actions_blocked: 1,
            consecutive_no_progress_turns: 0,
            user_visible_artifact: true,
        }
    }

    /// AC: roundtrip serializes & deserializes an AnvilScore-bearing snapshot.
    #[test]
    fn snapshot_with_anvil_score_roundtrips() {
        let snapshot = SessionSnapshot {
            last_anvil_score: Some(fixture_score()),
            ..SessionSnapshot::default()
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let decoded: SessionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.last_anvil_score, Some(fixture_score()));
    }

    /// AC: a snapshot whose last_anvil_score is None roundtrips with None.
    #[test]
    fn snapshot_with_no_anvil_score_roundtrips_none() {
        let snapshot = SessionSnapshot::default();
        let json = serde_json::to_string(&snapshot).unwrap();
        let decoded: SessionSnapshot = serde_json::from_str(&json).unwrap();
        assert!(decoded.last_anvil_score.is_none());
    }

    /// AC: legacy session.json (no last_anvil_score key) loads cleanly with
    /// last_anvil_score = None.
    #[test]
    fn legacy_session_without_anvil_score_loads_with_none() {
        let legacy = r#"{"mode_state":{"mode":"Act","active_plan_path":null},"messages":[],"checkpoints":[]}"#;
        let decoded: SessionSnapshot = serde_json::from_str(legacy).unwrap();
        assert!(decoded.last_anvil_score.is_none());
        assert_eq!(decoded.consecutive_no_progress_turns, 0);
    }

    /// AC: malformed last_anvil_score (e.g. wrong type for a usize) is
    /// silently lossy → None, the rest of the snapshot still loads.
    #[test]
    fn malformed_anvil_score_drops_to_none_session_loads() {
        let malformed = r#"{
            "mode_state":{"mode":"Act","active_plan_path":null},
            "messages":[],
            "checkpoints":[],
            "last_anvil_score":{"compile_error_count":"not-a-number"}
        }"#;
        let decoded: SessionSnapshot = serde_json::from_str(malformed).unwrap();
        assert!(decoded.last_anvil_score.is_none());
    }

    /// AC: a session whose last_anvil_score is malformed must NOT disappear
    /// from `iter_session_dirs` — discovery uses the same lossy wrapper as
    /// resume, so the directory survives and `anvil sessions list / clean`
    /// keeps showing it (S7-003).
    #[test]
    fn discovery_keeps_session_with_malformed_anvil_score() {
        let tmp = tempdir().unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        let session_dir = write_session_dir(tmp.path(), &id);
        let payload = format!(
            r#"{{"id":"{id}","workspace_key":"ws","mode_state":{{"mode":"Act","active_plan_path":null}},"messages":[],"checkpoints":[],"last_anvil_score":{{"compile_error_count":-1}}}}"#
        );
        fs::write(session_dir.join("session.json"), payload).unwrap();
        let entries = iter_session_dirs(tmp.path());
        assert_eq!(entries.len(), 1);
        assert!(entries[0].snapshot.last_anvil_score.is_none());
    }

    /// AC: oversized last_anvil_score (raw JSON over MAX_ANVIL_SCORE_RAW_BYTES)
    /// is dropped to None without parsing; the session itself stays loadable
    /// (DR4-003).
    #[test]
    fn oversized_anvil_score_drops_to_none_session_loads() {
        let mut big = String::new();
        big.push_str(r#"{"compile_error_count": 1, "filler":""#);
        big.push_str(&"x".repeat(MAX_ANVIL_SCORE_RAW_BYTES + 100));
        big.push_str(r#""}"#);
        let payload = format!(
            r#"{{"mode_state":{{"mode":"Act","active_plan_path":null}},"messages":[],"checkpoints":[],"last_anvil_score":{big}}}"#
        );
        let decoded: SessionSnapshot = serde_json::from_str(&payload).unwrap();
        assert!(decoded.last_anvil_score.is_none());
    }

    /// AC: oversized session.json (MAX_SESSION_JSON_BYTES) is rejected at
    /// load_or_new before parsing, in addition to discovery skipping it
    /// (DR4-003).
    #[test]
    fn oversized_session_json_rejected_by_load_or_new() {
        let dir = tempdir().unwrap();
        let id = "0199fe00-0000-7000-8000-456000000001";
        let session_dir = write_session_dir(dir.path(), id);
        let path = session_dir.join("session.json");
        // Write > MAX_SESSION_JSON_BYTES.
        let payload = vec![b'a'; (MAX_SESSION_JSON_BYTES + 1) as usize];
        fs::write(&path, payload).unwrap();
        let store = SessionStore::new(dir.path(), id, "ws-x");
        let result = store.load_or_new(false);
        assert!(result.is_err(), "expected oversized session to be rejected");
    }

    /// AC: huge numeric values (above MAX_ANVIL_SCORE_COUNT) are clamped /
    /// dropped in `sanitize`, so subsequent delta math doesn't panic
    /// (DR4-002).
    #[test]
    fn huge_numeric_values_are_sanitized() {
        // Use a value within usize range but above MAX_ANVIL_SCORE_COUNT for
        // the saturating clamp test (usize::MAX would be valid u64 JSON, but
        // the goal here is to verify clamp semantics, not parser limits).
        let payload = format!(
            r#"{{"mode_state":{{"mode":"Act","active_plan_path":null}},"messages":[],"checkpoints":[],"last_anvil_score":{{"compile_error_count":{},"unsafe_actions_blocked":{}}}}}"#,
            MAX_ANVIL_SCORE_COUNT + 1,
            MAX_ANVIL_SCORE_COUNT + 5,
        );
        let decoded: SessionSnapshot = serde_json::from_str(&payload).unwrap();
        let score = decoded.last_anvil_score.expect("score deserialized");
        // Optional count above cap → None.
        assert_eq!(score.compile_error_count, None);
        // Non-optional counter saturates at MAX_ANVIL_SCORE_COUNT.
        assert_eq!(score.unsafe_actions_blocked, MAX_ANVIL_SCORE_COUNT);
    }

    /// AC: i32::MIN delta values are dropped to None to keep delta math safe
    /// even when an attacker writes pathological values to session.json.
    #[test]
    fn i32_min_delta_dropped_to_none() {
        let payload = format!(
            r#"{{"mode_state":{{"mode":"Act","active_plan_path":null}},"messages":[],"checkpoints":[],"last_anvil_score":{{"compile_errors_delta":{},"test_failures_delta":{}}}}}"#,
            i32::MIN,
            i32::MIN,
        );
        let decoded: SessionSnapshot = serde_json::from_str(&payload).unwrap();
        let score = decoded.last_anvil_score.expect("score");
        assert_eq!(score.compile_errors_delta, None);
        assert_eq!(score.test_failures_delta, None);
    }

    /// AC: top-level JSON corruption is NOT silently downgraded to a fresh
    /// snapshot — it surfaces as a load error, the same way it did before
    /// AnvilScore was introduced (DR4-004).
    #[test]
    fn top_level_corruption_still_errors() {
        let dir = tempdir().unwrap();
        let id = "0199fe00-0000-7000-8000-456000000002";
        let session_dir = write_session_dir(dir.path(), id);
        fs::write(session_dir.join("session.json"), b"{not even json").unwrap();
        let store = SessionStore::new(dir.path(), id, "ws-x");
        assert!(store.load_or_new(false).is_err());
    }

    /// AC: top-level corruption keeps `iter_session_dirs` skipping the
    /// session (existing behavior, not regressed by AnvilScore).
    #[test]
    fn top_level_corruption_skipped_by_discovery() {
        let tmp = tempdir().unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        let session_dir = write_session_dir(tmp.path(), &id);
        fs::write(session_dir.join("session.json"), b"{broken").unwrap();
        let entries = iter_session_dirs(tmp.path());
        assert!(entries.is_empty());
    }

    /// AC: SessionStore::save / load_or_new roundtrip preserves
    /// `last_anvil_score` and `consecutive_no_progress_turns` (the only two
    /// of the new fields that are persisted).
    #[test]
    fn store_save_and_load_preserves_persisted_anvil_fields() {
        let dir = tempdir().unwrap();
        let id = "0199fe00-0000-7000-8000-456000000003";
        write_session_dir(dir.path(), id);
        let store = SessionStore::new(dir.path(), id, "ws-x");
        let snapshot = SessionSnapshot {
            last_anvil_score: Some(fixture_score()),
            consecutive_no_progress_turns: 7,
            // runtime-only fields: should not survive save/load
            unsafe_blocks_this_turn: 99,
            repo_edit_succeeded_this_turn: true,
            touched_files_at_turn_start: vec!["src/main.rs".into()],
            ..SessionSnapshot::default()
        };
        store.save(&snapshot).unwrap();
        let loaded = store.load_or_new(false).unwrap();
        assert_eq!(loaded.last_anvil_score, Some(fixture_score()));
        assert_eq!(loaded.consecutive_no_progress_turns, 7);
        // Skip-marked fields default after load.
        assert_eq!(loaded.unsafe_blocks_this_turn, 0);
        assert!(!loaded.repo_edit_succeeded_this_turn);
        assert!(loaded.touched_files_at_turn_start.is_empty());
    }
}
