//! Issue #453: Integration tests for `Active Precautions:` injection into the
//! Act-mode LLM prompt.
//!
//! These tests verify the full pipeline from
//! `WorkingMemory.active_precautions` → `select_precautions_for_prompt` →
//! `format_for_prompt_with_precautions` → `ConversationMessage::system` body
//! that the agent loop hands to the Ollama client.
//!
//! The tests are in-process and do not require a live Ollama server. The
//! design (DR2-001) allows asserting either the `ollama.chat.request` or
//! `ollama.generate.request` event payload; here we directly assert the
//! constructed system message body, which is what either event would carry
//! in its `messages` field.

use anvil::agent::loop_run::select_precautions_for_prompt;
use anvil::modes::plan_act::ExecutionMode;
use anvil::session::precaution::{Precaution, PrecautionSource, PrecautionStatus, Severity};
use anvil::session::store::{ConversationMessage, WorkingMemory};
use std::path::PathBuf;
use tempfile::tempdir;

fn make_precaution(text: &str, severity: Severity) -> Precaution {
    Precaution {
        id: format!("manual-{text}"),
        source: PrecautionSource::Manual,
        severity,
        text: text.to_string(),
        applies_to: Vec::new(),
        status: PrecautionStatus::Active,
        retired_reason: None,
    }
}

/// Build the `system` ConversationMessage exactly as `working_memory_message`
/// would (Issue #453): select for the given mode + signals, then render
/// through the new `format_for_prompt_with_precautions` entry point.
fn build_system_message(
    wm: &WorkingMemory,
    mode: ExecutionMode,
    suspected_files: Option<&[PathBuf]>,
) -> Option<ConversationMessage> {
    let selected = select_precautions_for_prompt(
        &wm.active_precautions,
        mode,
        &wm.touched_files,
        suspected_files,
    );
    wm.format_for_prompt_with_precautions(&selected)
        .map(ConversationMessage::system)
}

#[test]
fn act_mode_request_includes_active_precautions_section() {
    let dir = tempdir().unwrap();
    let mut wm = WorkingMemory::default();
    wm.set_active_task(Some("ship feature X".to_string()));
    wm.add_precaution(
        make_precaution("Avoid sed -i for in-place edits", Severity::High),
        dir.path(),
    );

    let msg = build_system_message(&wm, ExecutionMode::Act, None)
        .expect("Act mode with active precaution must yield a system message");
    assert_eq!(msg.role, "system");
    // The exact body that ollama.chat.request / ollama.generate.request would
    // record under messages[].content for this system entry.
    assert!(
        msg.content.contains("Active Precautions:"),
        "expected Active Precautions header in Act mode, got: {}",
        msg.content
    );
    assert!(
        msg.content
            .contains("- [high] Avoid sed -i for in-place edits"),
        "expected high-severity precaution bullet, got: {}",
        msg.content
    );
}

#[test]
fn plan_mode_request_excludes_active_precautions_section() {
    let dir = tempdir().unwrap();
    let mut wm = WorkingMemory::default();
    wm.set_active_task(Some("explore design alternatives".to_string()));
    wm.add_precaution(
        make_precaution("Plan-mode hidden constraint", Severity::High),
        dir.path(),
    );

    let msg = build_system_message(&wm, ExecutionMode::Plan, None)
        .expect("Plan mode still emits the working-memory message for active_task");
    assert_eq!(msg.role, "system");
    // Active task survives, but the Active Precautions section is suppressed
    // in Plan mode (design judgment #2).
    assert!(
        msg.content
            .contains("Active task: explore design alternatives"),
        "expected active task to remain in Plan mode, got: {}",
        msg.content
    );
    assert!(
        !msg.content.contains("Active Precautions:"),
        "Plan mode must not surface Active Precautions section, got: {}",
        msg.content
    );
    assert!(
        !msg.content.contains("Plan-mode hidden constraint"),
        "Plan mode must not leak the precaution text, got: {}",
        msg.content
    );
}

#[test]
fn no_active_precaution_omits_section_entirely() {
    let mut wm = WorkingMemory::default();
    wm.set_active_task(Some("simple task".to_string()));
    // No precautions added.

    let msg = build_system_message(&wm, ExecutionMode::Act, None)
        .expect("active_task alone still yields a working-memory message");
    assert!(
        !msg.content.contains("Active Precautions:"),
        "section must be hidden when no active precaution exists, got: {}",
        msg.content
    );
    assert!(
        msg.content.contains("Active task: simple task"),
        "active task must remain visible, got: {}",
        msg.content
    );
}

#[test]
fn act_mode_request_respects_severity_order() {
    // AC: Severity::High が優先される
    let dir = tempdir().unwrap();
    let mut wm = WorkingMemory::default();
    wm.add_precaution(make_precaution("low-A", Severity::Low), dir.path());
    wm.add_precaution(make_precaution("med-B", Severity::Medium), dir.path());
    wm.add_precaution(make_precaution("high-C", Severity::High), dir.path());

    let msg = build_system_message(&wm, ExecutionMode::Act, None).expect("system message");
    let high_pos = msg
        .content
        .find("- [high] high-C")
        .expect("high bullet present");
    let med_pos = msg
        .content
        .find("- [medium] med-B")
        .expect("medium bullet present");
    let low_pos = msg
        .content
        .find("- [low] low-A")
        .expect("low bullet present");
    assert!(
        high_pos < med_pos && med_pos < low_pos,
        "expected high → medium → low ordering, got: {}",
        msg.content
    );
}

#[test]
fn act_mode_request_uses_suspected_files_for_relevance() {
    // To exercise relevance scoring end-to-end we must keep `applies_to`
    // populated past `canonicalize_applies_to`. That helper drops relative
    // multi-component paths whose `canonicalize()` fails (CB-004), so the
    // files have to actually exist on disk in the workspace temp dir.
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(dir.path().join("src/other.rs"), "// other\n").unwrap();

    let mut wm = WorkingMemory::default();
    let mut p1 = make_precaution("touches-other", Severity::Medium);
    p1.applies_to = vec![PathBuf::from("src/other.rs")];
    let mut p2 = make_precaution("touches-suspect", Severity::Medium);
    p2.applies_to = vec![PathBuf::from("src/main.rs")];
    wm.add_precaution(p1, dir.path());
    wm.add_precaution(p2, dir.path());

    let suspected = vec![PathBuf::from("src/main.rs")];
    let msg =
        build_system_message(&wm, ExecutionMode::Act, Some(&suspected)).expect("system message");

    let suspect_pos = msg
        .content
        .find("touches-suspect")
        .expect("suspected bullet present");
    let other_pos = msg
        .content
        .find("touches-other")
        .expect("other bullet present");
    assert!(
        suspect_pos < other_pos,
        "suspected hit must outrank unrelated within same severity, got: {}",
        msg.content
    );
}
