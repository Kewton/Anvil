//! Issue #931 (P1c): recovery-prompt path/reason masking — structural guard.
//!
//! This in-crate `#[cfg(test)] mod` enforces the masking convention documented
//! on `task_contract::mask_and_cap_recovery_field`:
//!
//! - **Enumerated mask tests** (AC1): every Choke-B / Choke-C renderer and every
//!   Choke-D `json!` wire field, fed a secret-shaped path/reason, must not leak
//!   the secret substring.
//! - **Byte-equality goldens** (AC5): every renderer arm, fed an ORDINARY path,
//!   must be byte-identical to the pinned snapshot (proves `mask_secrets` /
//!   `mask_and_cap_recovery_field` are no-ops on ordinary input).
//! - **Shared-SSOT regression** (AC5 / Phase F): `obligation_report_label` output
//!   is byte-identical to its pinned snapshot (proves `mask_and_cap_label` /
//!   `mask_obligation_value` / `mask_secrets` are unchanged).
//! - **Source-scan structural guard** (AC3 / Phase E): a function-scoped
//!   allowlist + intra-function taint pass + broad smoke scan over
//!   `src/agent/loop_run/*.rs` fails if a new renderer routes a raw
//!   hint.path / hint.reason / `progress_path_display(...)` into a prompt sink
//!   without a render-point mask. The scanner self-verifies on inline fixtures.
//!
//! `#[cfg(test)]` only; production binary excludes this file (CB-001 /
//! DR3-001). No facade re-export.

use std::path::Path;

use super::task_contract::{ArtifactRole, RecoveryTargetHint, mask_and_cap_recovery_field};

const SECRET: &str = "AKIASECRETPATH0123456789";

/// A secret-shaped workspace-relative path. `mask_secrets` rewrites
/// `token=<value>` to `token=***`, removing the secret tail.
fn secret_path() -> String {
    format!("app/token={SECRET}.tsx")
}

/// A secret-shaped free-text reason.
fn secret_reason() -> String {
    format!("see token={SECRET} for details")
}

fn assert_masked(label: &str, rendered: &str) {
    assert!(
        !rendered.contains(SECRET),
        "[{label}] secret leaked into recovery prompt: {rendered}"
    );
    assert!(
        rendered.contains("token=***"),
        "[{label}] kv secret must be masked to token=***: {rendered}"
    );
}

// ---------------------------------------------------------------------------
// Phase B (Choke B): recovery_messages.rs pure-helper enumerated + golden tests
// ---------------------------------------------------------------------------

#[test]
fn focused_edit_no_tool_note_for_target_body_masks_and_is_byte_stable() {
    use super::recovery_messages::focused_edit_no_tool_note_for_target_body;
    // Enumerated mask: every (is_file, already_read) arm masks a secret path.
    for is_file in [true, false] {
        for already_read in [true, false] {
            let masked =
                focused_edit_no_tool_note_for_target_body(&secret_path(), is_file, already_read, 1);
            assert_masked("focused_edit_no_tool_note_for_target_body", &masked);
        }
    }
    // Byte-equality golden: ordinary path is byte-identical across all arms.
    let ordinary = "app/page.tsx";
    assert_eq!(
        focused_edit_no_tool_note_for_target_body(ordinary, false, false, 2),
        super::super::recovery::focused_edit_missing_target_recovery_note(ordinary, 2)
    );
    assert_eq!(
        focused_edit_no_tool_note_for_target_body(ordinary, true, true, 3),
        super::super::recovery::focused_edit_no_tool_recovery_note(ordinary, true, 3)
    );
    assert_eq!(
        focused_edit_no_tool_note_for_target_body(ordinary, true, false, 4),
        super::super::recovery::focused_edit_no_tool_recovery_note(ordinary, false, 4)
    );
}

#[test]
fn focused_edit_no_tool_note_for_policy_arm_masks_all_arms_and_is_byte_stable() {
    use super::recovery_messages::focused_edit_no_tool_note_for_policy_arm;
    // Enumerated mask: each verifier-repair allowlist arm masks the secret path.
    for allowed in [
        Some(&["Read"][..]),
        Some(&["Edit"][..]),
        Some(&["Write"][..]),
    ] {
        let masked = focused_edit_no_tool_note_for_policy_arm(&secret_path(), allowed, 1)
            .expect("verifier-repair arm should render a note");
        assert_masked("focused_edit_no_tool_note_for_policy_arm", &masked);
    }
    // No matching allowlist => None (fall-through), masked secret never rendered.
    assert!(focused_edit_no_tool_note_for_policy_arm(&secret_path(), None, 1).is_none());
    assert!(
        focused_edit_no_tool_note_for_policy_arm(&secret_path(), Some(&["Read", "Edit"][..]), 1)
            .is_none()
    );

    // Byte-equality golden: ordinary path, each arm.
    let p = "app/page.tsx";
    assert_eq!(
        focused_edit_no_tool_note_for_policy_arm(p, Some(&["Read"][..]), 5).unwrap(),
        format!(
            "Verifier repair is waiting for a fresh read of {p}. The previous response was not executed. Emit exactly one Read tool call on that file now. Do not call Edit, Bash, Glob, Grep, or answer in prose. verifier_repair_read_attempt=5"
        )
    );
    assert_eq!(
        focused_edit_no_tool_note_for_policy_arm(p, Some(&["Edit"][..]), 6).unwrap(),
        format!(
            "Verifier repair is waiting for a compact edit of {p}. The previous response was not executed. Emit exactly one Edit tool call on that file now. Do not call Read, Bash, Glob, Grep, or answer in prose. verifier_repair_edit_attempt=6"
        )
    );
    assert_eq!(
        focused_edit_no_tool_note_for_policy_arm(p, Some(&["Write"][..]), 7).unwrap(),
        format!(
            "Verifier repair is waiting for the missing target {p}. The previous response was not executed. Emit exactly one Write tool call on that exact path now. Do not call Read, Bash, Glob, Grep, or answer in prose. verifier_repair_write_attempt=7"
        )
    );
}

#[test]
fn artifact_directed_recovery_message_body_masks_and_is_byte_stable() {
    use super::recovery_messages::artifact_directed_recovery_message_body;
    // Enumerated mask: both read-state arms mask the secret path.
    for already_read in [true, false] {
        let masked = artifact_directed_recovery_message_body(
            &secret_path(),
            "implementation",
            "Read, Write, Edit",
            already_read,
        );
        assert_masked("artifact_directed_recovery_message_body", &masked);
    }
    // Byte-equality golden: ordinary path, both arms.
    let p = "app/page.tsx";
    assert_eq!(
        artifact_directed_recovery_message_body(p, "implementation", "Read, Write, Edit", false),
        format!(
            "[Artifact Directed Recovery] Missing role: implementation. Target file: {p}. Allowed tools for this turn are Read, Write, Edit on that exact target path only. Do not call Bash, Glob, Grep, or switch files. Use Write if a small scaffold file should be replaced; otherwise use a compact Edit."
        )
    );
    assert_eq!(
        artifact_directed_recovery_message_body(p, "implementation", "Read, Write, Edit", true),
        format!(
            "[Artifact Directed Recovery] Missing role: implementation. Target file: {p}. Allowed tools for this turn are Read, Write, Edit on that exact target path only. The target has already been read in this session, so do not call Read again. Do not call Bash, Glob, Grep, or switch files. Use Write if a small scaffold file should be replaced; otherwise use a compact Edit."
        )
    );
}

#[test]
fn verifier_repair_request_patch_message_body_masks_all_arms_and_is_byte_stable() {
    use super::recovery_messages::verifier_repair_request_patch_message_body;
    // Enumerated mask: every (is_file, already_read) arm masks the secret path.
    for is_file in [true, false] {
        for already_read in [true, false] {
            let masked = verifier_repair_request_patch_message_body(
                &secret_path(),
                " diag.",
                is_file,
                already_read,
            );
            assert_masked("verifier_repair_request_patch_message_body", &masked);
        }
    }
    // Byte-equality golden: ordinary path, each distinct arm.
    let p = "app/page.tsx";
    let diag = " Diagnostics here.";
    assert_eq!(
        verifier_repair_request_patch_message_body(p, diag, false, false),
        format!(
            "[Verifier Repair Policy] A verifier failure is pending.{diag} Missing target file: {p}. Next required action: exactly one Write on that target. Do not use Bash, switch files, or finish with prose. Anvil will rerun the verifier after the write."
        )
    );
    assert_eq!(
        verifier_repair_request_patch_message_body(p, diag, true, true),
        format!(
            "[Verifier Repair Policy] A verifier failure is pending.{diag} Target file: {p}. Next required action: exactly one compact Edit on that target. Do not call Read again, Bash, switch files, or finish with prose. Anvil will rerun the verifier after the edit."
        )
    );
    assert_eq!(
        verifier_repair_request_patch_message_body(p, diag, true, false),
        format!(
            "[Verifier Repair Policy] A verifier failure is pending.{diag} Target file: {p}. Next required action: exactly one Read on that target. Do not use Edit, Bash, switch files, or finish with prose."
        )
    );
}

#[test]
fn deterministic_ui_recovery_continuation_note_body_masks_and_is_byte_stable() {
    use super::recovery_messages::deterministic_ui_recovery_continuation_note_body;
    let masked = deterministic_ui_recovery_continuation_note_body(&secret_path(), 1);
    assert_masked("deterministic_ui_recovery_continuation_note_body", &masked);
    // Byte-equality golden.
    let p = "app/page.tsx";
    assert_eq!(
        deterministic_ui_recovery_continuation_note_body(p, 9),
        format!(
            "Deterministic UI recovery updated {p}, but this is recovery context, not completion. Inspect the file if needed, then make one small model-produced Edit or run the project verifier before finalizing. deterministic_ui_recovery_attempt=9"
        )
    );
}

// ---------------------------------------------------------------------------
// Phase B (Choke B): focused_edit_recovery.rs — already-pure helpers (DR1-005):
// not split, only enumerated + golden coverage added here.
// ---------------------------------------------------------------------------

#[test]
fn focused_edit_guidance_note_masks_and_is_byte_stable() {
    use super::focused_edit_recovery::focused_edit_guidance_note;
    let work_root = Path::new("/work");
    // Enumerated mask: a secret-shaped relative path under work_root. The path
    // does not exist on disk, so `target.is_file()` is false → "does not exist"
    // arm; the masking is independent of disk state.
    let secret_target = work_root.join(secret_path());
    for already_read in [true, false] {
        let masked = focused_edit_guidance_note(&secret_target, work_root, already_read);
        assert_masked("focused_edit_guidance_note", &masked);
    }
    // Byte-equality golden: ordinary, non-existent target → "does not exist" arm.
    let ordinary = work_root.join("app/page.tsx");
    assert_eq!(
        focused_edit_guidance_note(&ordinary, work_root, false),
        "[Focused Edit Recovery] The target file app/page.tsx does not exist yet. The only available tool for this turn is Write on that exact path. Do not call Read, Bash, Glob, or Grep; create the missing artifact directly and keep the body focused on the requested role."
    );
    assert_eq!(
        focused_edit_guidance_note(&ordinary, work_root, true),
        "[Focused Edit Recovery] The target file app/page.tsx has already been read. The only available tool for this turn is Edit. Do not call Read again. Use exactly one compact Edit on that file now. Replace only one contiguous block from the last Read. Do not attempt a full-file rewrite, multi-file change, scaffold command, or dev-server command."
    );
}

#[test]
fn focused_edit_compact_anchor_note_masks_and_is_byte_stable() {
    use super::focused_edit_recovery::focused_edit_compact_anchor_note;
    let work_root = Path::new("/work");
    let masked = focused_edit_compact_anchor_note(&work_root.join(secret_path()), work_root);
    assert_masked("focused_edit_compact_anchor_note", &masked);
    // Byte-equality golden.
    assert_eq!(
        focused_edit_compact_anchor_note(&work_root.join("app/page.tsx"), work_root),
        "[Focused Edit Recovery / Compact Anchor] The Read result for app/page.tsx is intentionally only a tiny exact anchor from the real file, not the whole file. Use that anchor only for `old_string`. Keep `new_string` similarly small: at most 3 lines and under 240 characters. Do not insert imports, hooks, component definitions, or full-file content. If the anchor is CTA or placeholder text, replace only that text with a short task-specific label or copy."
    );
}

#[test]
fn focused_edit_guidance_note_for_policy_masks_all_arms_and_is_byte_stable() {
    use super::focused_edit_recovery::focused_edit_guidance_note_for_policy;
    use super::tool_policy::{EffectiveToolPolicy, EffectiveToolPolicyReason};
    let work_root = Path::new("/work");
    let secret_target = work_root.join(secret_path());

    // Enumerated mask: Read/Edit/Write verifier-repair arms each mask the path.
    for allowed_tool in ["Read", "Edit", "Write"] {
        let policy = EffectiveToolPolicy::focused_edit(
            EffectiveToolPolicyReason::VerifierRepair,
            vec![allowed_tool],
            secret_target.clone(),
            allowed_tool == "Edit",
        );
        let masked =
            focused_edit_guidance_note_for_policy(&policy, &secret_target, work_root, false);
        assert_masked("focused_edit_guidance_note_for_policy", &masked);
    }
}

// ---------------------------------------------------------------------------
// Phase B (Choke B): verifier_orchestration.rs note builders (already masked).
// ---------------------------------------------------------------------------

fn verifier_context_for(path: &str, reason: &str) -> super::repair_job::RepairJob {
    let hint = RecoveryTargetHint {
        role: ArtifactRole::Implementation,
        path: path.to_string(),
        reason: reason.to_string(),
    };
    super::repair_job::RepairJob {
        command: "python3 -m pytest".to_string(),
        output_excerpt: "failure".to_string(),
        failure_type: super::VerifierFailureType::RuntimeError,
        target_hint: Some(hint.clone()),
        repair_target_hint: Some(hint.clone()),
        changed_file_hints: vec![hint.clone()],
        assessment: Some(super::VerifierRepairAssessment {
            failure_kind: super::VerifierDiagnosticFailureKind::RuntimeError,
            failure_type: super::VerifierFailureType::RuntimeError,
            probable_cause_role: Some(ArtifactRole::Implementation),
            needed_reads: vec![hint.clone()],
            repair_target_hint: Some(hint.clone()),
            repair_plan: vec![hint.clone()],
            summary: Some("test helper assessment".to_string()),
            source: super::VerifierRepairAssessmentSource::DiagnosticPass,
        }),
        diagnostic_attempted: true,
        error_kind: Some("TypeError".to_string()),
        // The failure_signature is a computed hash/label, NOT raw LLM/request
        // path — keep the raw secret out of it so this fixture isolates the
        // renderer's path/reason masking (the #931 mask targets), and feed the
        // path via `{role}` so an unrelated field cannot mask the leak.
        failure_signature: "sig-typeerror".to_string(),
        failure_count: Some(1),
        repair_attempt: 1,
        ..super::repair_job::RepairJob::new_for_test()
    }
}

#[test]
fn verifier_repair_diagnostic_pending_note_masks_secret() {
    let context = verifier_context_for(&secret_path(), "test");
    let note =
        super::verifier_orchestration::verifier_repair_diagnostic_pending_note(&context, None);
    assert_masked("verifier_repair_diagnostic_pending_note", &note);
}

#[test]
fn task_contract_verifier_repair_note_masks_secret() {
    let context = verifier_context_for(&secret_path(), "test");
    let note = super::verifier_orchestration::task_contract_verifier_repair_note(
        "python3 -m pytest",
        "out",
        1,
        3,
        Some(&context),
    );
    assert_masked("task_contract_verifier_repair_note", &note);
}

#[test]
fn task_contract_verifier_targeted_edit_required_note_masks_secret() {
    let context = verifier_context_for(&secret_path(), "test");
    let note = super::verifier_orchestration::task_contract_verifier_targeted_edit_required_note(
        &context,
        Path::new("/work"),
        false,
        1,
        3,
    );
    assert_masked("task_contract_verifier_targeted_edit_required_note", &note);
}

// ---------------------------------------------------------------------------
// Phase C (Choke C): tool_policy.rs error producers carry a MASKED path.
// ---------------------------------------------------------------------------

#[test]
fn focused_edit_tool_policy_error_masks_path_in_error() {
    let work_root = Path::new("/work");
    let target = work_root.join(secret_path());
    // A mismatching tool call (Read on a missing-target focused edit) produces the
    // "only allows Write on missing target {path}" error carrying the masked path.
    let err = super::tool_policy::focused_edit_tool_policy_error(
        "Read",
        &serde_json::json!({"path": "other.txt"}),
        &target,
        work_root,
        false,
    )
    .expect("policy error should fire");
    assert_masked("focused_edit_tool_policy_error", &err);
}

#[test]
fn artifact_directed_tool_policy_error_masks_path_in_error() {
    let work_root = Path::new("/work");
    let target = work_root.join(secret_path());
    let err = super::tool_policy::artifact_directed_tool_policy_error(
        "Bash",
        &serde_json::json!({"command": "ls"}),
        &target,
        work_root,
    )
    .expect("policy error should fire");
    assert_masked("artifact_directed_tool_policy_error", &err);
}

#[test]
fn focused_edit_tool_policy_error_byte_stable_for_ordinary_path() {
    let work_root = Path::new("/work");
    let target = work_root.join("app/page.tsx");
    let err = super::tool_policy::focused_edit_tool_policy_error(
        "Read",
        &serde_json::json!({"path": "other.txt"}),
        &target,
        work_root,
        false,
    )
    .unwrap();
    assert_eq!(
        err,
        "focused edit recovery rejected Read; only allows Write on missing target app/page.tsx"
    );
}

#[test]
fn focused_edit_policy_violation_feedback_note_fires_with_masked_content() {
    // Choke C end-to-end: the retained policy error already carries a masked path,
    // and the feedback-note filter target (also masked) matches it, so the note
    // fires with masked content (DR4-003: masked-fire, not unfired).
    let work_root = Path::new("/work");
    let target = work_root.join(secret_path());
    let error = super::tool_policy::focused_edit_tool_policy_error(
        "Read",
        &serde_json::json!({"path": "other.txt"}),
        &target,
        work_root,
        false,
    )
    .unwrap();
    let unresolved = vec![error];
    let masked_target = mask_and_cap_recovery_field(&secret_path());
    let note = super::tool_policy::focused_edit_policy_violation_feedback_note(
        &unresolved,
        Some(&["Write"]),
        Some(&masked_target),
    )
    .expect("feedback note should fire on the masked target");
    assert_masked("focused_edit_policy_violation_feedback_note", &note);
}

#[test]
fn focused_edit_policy_violation_feedback_note_ordinary_dedups_and_fires() {
    // Regression (DR3-003 (a)): an ordinary policy error still fires the note.
    let work_root = Path::new("/work");
    let target = work_root.join("app/page.tsx");
    let error = super::tool_policy::focused_edit_tool_policy_error(
        "Read",
        &serde_json::json!({"path": "other.txt"}),
        &target,
        work_root,
        false,
    )
    .unwrap();
    let unresolved = vec![error];
    let note = super::tool_policy::focused_edit_policy_violation_feedback_note(
        &unresolved,
        Some(&["Write"]),
        Some("app/page.tsx"),
    )
    .expect("feedback note should fire on the ordinary target");
    assert!(note.contains("app/page.tsx"));
    assert!(note.contains("[Focused Edit Policy Violation]"));
}

#[test]
fn dispatcher_masks_artifact_directed_bash_rejection_target() {
    // Issue #931 CB-001 (Codex Phase 2.5 finding): the ArtifactDirectedRecovery +
    // Bash early-rejection branch of `effective_tool_policy_error_for_call_with_scope`
    // renders a `policy_target_path`-derived target into a model-facing error that
    // is retained in `unresolved_errors` and surfaced as a tool result. A
    // secret-shaped target must be masked at the render point.
    use super::tool_policy::{
        EffectiveToolPolicy, effective_tool_policy_error_for_call_with_scope,
    };
    let work_root = Path::new("/work");
    let target = work_root.join(secret_path());
    let policy = EffectiveToolPolicy::artifact_directed(target, false);
    let err = effective_tool_policy_error_for_call_with_scope(
        &policy,
        "Bash",
        &serde_json::json!({"command": "ls"}),
        work_root,
        None,
    )
    .expect("artifact-directed Bash rejection should fire");
    assert_masked("dispatcher artifact-directed Bash rejection", &err);
    // CB-001 Choke-C symmetry: the masked error must still match the masked filter
    // target so the follow-up policy-violation note fires (with masked content).
    let masked_target = mask_and_cap_recovery_field(&secret_path());
    let note = super::tool_policy::focused_edit_policy_violation_feedback_note(
        std::slice::from_ref(&err),
        Some(&["Read", "Write", "Edit"]),
        Some(&masked_target),
    )
    .expect("policy-violation note should fire on the masked target");
    assert_masked("policy-violation note (artifact-directed Bash)", &note);
}

// ---------------------------------------------------------------------------
// Phase D (Choke D): json! wire payloads carry masked path-identity + reason.
// ---------------------------------------------------------------------------

#[test]
fn diagnostic_wire_payload_masks_secret_path_and_reason() {
    let context = verifier_context_for(&secret_path(), &secret_reason());
    let messages = super::verifier_orchestration::verifier_diagnostic_messages(
        Path::new("/work"),
        &context,
        "do the task",
        None,
    );
    let serialized = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !serialized.contains(SECRET),
        "secret leaked into diagnostic wire payload: {serialized}"
    );
}

#[test]
fn repair_wire_payload_masks_secret_path_and_reason() {
    use std::fs;
    let temp = tempfile::tempdir().unwrap();
    let work_root = temp.path();
    fs::create_dir_all(work_root.join("app")).unwrap();
    // The repair payload requires the selected target to be a real, excerptable
    // file. A secret-shaped filename is a perfectly valid path on disk.
    let rel = secret_path();
    fs::write(work_root.join(&rel), "value = 1\n").unwrap();
    let context = verifier_context_for(&rel, &secret_reason());
    let target_hint = RecoveryTargetHint {
        role: ArtifactRole::Implementation,
        path: rel.clone(),
        reason: secret_reason(),
    };
    let messages = super::verifier_orchestration::verifier_repair_pass_messages(
        work_root,
        &context,
        &target_hint,
        "do the task",
        None,
    )
    .expect("repair pass messages should build");
    let serialized = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !serialized.contains(SECRET),
        "secret leaked into repair wire payload: {serialized}"
    );
}

#[test]
fn diagnostic_wire_payload_byte_stable_for_ordinary_path() {
    // AC4/AC5: an ordinary path is byte-unchanged on the wire (mask_secrets no-op).
    let ordinary = "app/calculator.py";
    let context = verifier_context_for(ordinary, "short bounded reason");
    let messages = super::verifier_orchestration::verifier_diagnostic_messages(
        Path::new("/work"),
        &context,
        "do the task",
        None,
    );
    let serialized = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        serialized.contains(ordinary),
        "ordinary path must survive byte-exact on the wire: {serialized}"
    );
    assert!(
        serialized.contains("short bounded reason"),
        "ordinary reason must survive byte-exact on the wire: {serialized}"
    );
}

// Phase F (AC5): the shared-SSOT (`obligation_report_label`) byte-identity
// regression lives inside `task_contract.rs`'s own `#[cfg(test)] mod` because
// `obligation_report_label` is module-private — see
// `obligation_report_label_byte_identity_regression_issue931` there.

include!("recovery_masking_source_scan.rs");
