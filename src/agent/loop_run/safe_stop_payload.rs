use crate::session::feedback::mask_secrets;
use crate::session::store::ConversationMessage;

use super::VerifierFailureType;
use super::repair_job::{SafeStopReport, StopReason};

/// SSOT for the bounded `agent.safe_stop.report` payload. The size cap is
/// enforced by `build_safe_stop_payload`; oversize payloads are
/// deterministically trimmed and `"truncated": true` is set on the payload.
pub(super) const SAFE_STOP_REPORT_EVENT_MAX_BYTES: usize = 4096;

/// Collect a compact list of recent action labels (newest-last) from the
/// conversation message stream for use as the `actual_actions_raw` input to
/// `SafeStopContext`.
///
/// Labels contain only tool names and a stable masked hash, never raw paths or
/// commands.
pub(super) fn collect_recent_action_labels(messages: &[ConversationMessage]) -> Vec<String> {
    use crate::logging::stable_path_hash;

    const SCAN_LIMIT: usize = 24;
    let mut out: Vec<String> = Vec::new();
    let scan_start = messages.len().saturating_sub(SCAN_LIMIT);
    for msg in messages.iter().skip(scan_start) {
        if msg.role != "assistant" {
            continue;
        }
        for tool_call in msg.tool_calls.iter() {
            let name = tool_call.name.as_str();
            if !matches!(name, "Read" | "Write" | "Edit" | "Bash") {
                continue;
            }
            let raw_detail = tool_call
                .arguments
                .get("path")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    tool_call
                        .arguments
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                })
                .unwrap_or("");
            let label = if raw_detail.is_empty() {
                name.to_string()
            } else {
                let hash = stable_path_hash(&mask_secrets(raw_detail));
                format!("{name} <{hash}>")
            };
            out.push(label);
        }
    }
    out
}

/// Render a `SafeStopReport` to a `serde_json::Value` suitable for
/// `log_llm_event`. Bounded by `SAFE_STOP_REPORT_EVENT_MAX_BYTES`.
pub(super) fn build_safe_stop_payload(report: &SafeStopReport) -> serde_json::Value {
    let mut payload = render_safe_stop_payload(report, false);
    if serialized_byte_len(&payload) <= SAFE_STOP_REPORT_EVENT_MAX_BYTES {
        return payload;
    }
    payload = render_safe_stop_payload_trimmed(report, TrimTier::Tier1);
    if serialized_byte_len(&payload) <= SAFE_STOP_REPORT_EVENT_MAX_BYTES {
        return payload;
    }
    payload = render_safe_stop_payload_trimmed(report, TrimTier::Tier2);
    if serialized_byte_len(&payload) <= SAFE_STOP_REPORT_EVENT_MAX_BYTES {
        return payload;
    }
    payload = render_safe_stop_payload_trimmed(report, TrimTier::Tier3);
    if serialized_byte_len(&payload) <= SAFE_STOP_REPORT_EVENT_MAX_BYTES {
        return payload;
    }
    enforce_safe_stop_payload_hard_cap(report)
}

fn enforce_safe_stop_payload_hard_cap(report: &SafeStopReport) -> serde_json::Value {
    let serialized_session_id = sanitize_safe_stop_mandatory_string(&report.session_id);
    let payload = serde_json::json!({
        "session_id": serialized_session_id,
        "turn_index": report.turn_index,
        "stop_reason": report.stop_reason.as_str(),
        "failure_type": report.failure_type.as_str(),
        "failure_signature": "",
        "command": "",
        "output_excerpt": "",
        "current_role": serde_json::Value::Null,
        "expected_target": serde_json::Value::Null,
        "actual_actions": serde_json::json!([]),
        "diagnostic_target_missing_reason": serde_json::Value::Null,
        "blocker_class": safe_stop_blocker_class(report),
        "authority_status": safe_stop_authority_status(report),
        "next_user_action": safe_stop_next_user_action(report),
        "exhausted_attempts_summary": serde_json::Value::Null,
        "owned_test_artifacts": serde_json::json!([]),
        "truncated": true,
    });
    debug_assert!(
        serialized_byte_len(&payload) <= SAFE_STOP_REPORT_EVENT_MAX_BYTES,
        "safe_stop hard-cap projection exceeded {SAFE_STOP_REPORT_EVENT_MAX_BYTES} bytes"
    );
    payload
}

fn sanitize_safe_stop_mandatory_string(raw: &str) -> String {
    const SAFE_STOP_MANDATORY_STRING_MAX_CHARS: usize = 240;
    if raw.chars().count() <= SAFE_STOP_MANDATORY_STRING_MAX_CHARS {
        return raw.to_string();
    }
    raw.chars()
        .take(SAFE_STOP_MANDATORY_STRING_MAX_CHARS)
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TrimTier {
    Tier1,
    Tier2,
    Tier3,
}

fn serialized_byte_len(payload: &serde_json::Value) -> usize {
    serde_json::to_vec(payload).map(|v| v.len()).unwrap_or(0)
}

fn render_safe_stop_payload(report: &SafeStopReport, truncated: bool) -> serde_json::Value {
    let exhausted = report.exhausted_attempts_summary.as_ref().map(|s| {
        serde_json::json!({
            "total": s.total,
            "per_cluster": s.per_cluster.iter().map(|(k, roles)| {
                serde_json::json!([k, roles])
            }).collect::<Vec<_>>(),
            "last_repair_hypothesis": s.last_repair_hypothesis,
        })
    });
    serde_json::json!({
        "session_id": report.session_id,
        "turn_index": report.turn_index,
        "stop_reason": report.stop_reason.as_str(),
        "failure_type": report.failure_type.as_str(),
        "failure_signature": report.failure_signature,
        "command": report.command,
        "output_excerpt": report.output_excerpt,
        "current_role": report.current_role.map(|r| r.label()),
        "expected_target": report.expected_target,
        "actual_actions": report.actual_actions,
        "diagnostic_target_missing_reason": report
            .diagnostic_target_missing_reason
            .map(|r| r.as_str()),
        "blocker_class": safe_stop_blocker_class(report),
        "authority_status": safe_stop_authority_status(report),
        "next_user_action": safe_stop_next_user_action(report),
        "exhausted_attempts_summary": exhausted,
        "owned_test_artifacts": report.owned_test_artifacts,
        "truncated": truncated,
    })
}

fn safe_stop_blocker_class(report: &SafeStopReport) -> &'static str {
    match report.stop_reason {
        StopReason::ArtifactCompletionFailed => "artifact_completion",
        StopReason::VerifierMissing => "missing_verifier_or_setup",
        StopReason::VerifierWeak => "weak_or_unbound_verifier",
        StopReason::DiagnosticTargetMissing => "diagnostic_target_selection",
        StopReason::RepairExhausted => "repair_convergence",
        StopReason::VerifierFailedSafeStop => match report.failure_type {
            VerifierFailureType::CompileOrSyntax => "compile_or_syntax_failure",
            VerifierFailureType::ImportOrDependency => "import_or_dependency_failure",
            VerifierFailureType::RuntimeError => "runtime_failure",
            VerifierFailureType::AssertionFailure => "assertion_authority",
            VerifierFailureType::MissingVerifierOrConfig => "missing_verifier_or_setup",
            VerifierFailureType::DiagnosticTargetMissing => "diagnostic_target_selection",
            VerifierFailureType::RepairExhausted => "repair_convergence",
            VerifierFailureType::Unknown => "unclassified_verifier_failure",
        },
    }
}

fn safe_stop_authority_status(report: &SafeStopReport) -> &'static str {
    match (report.stop_reason, report.failure_type) {
        (StopReason::VerifierFailedSafeStop, VerifierFailureType::AssertionFailure) => {
            "insufficient_external_authority_for_expected_value"
        }
        (StopReason::RepairExhausted, _) => "repair_authority_or_progress_not_established",
        (StopReason::VerifierWeak, _) => "verifier_not_authoritative",
        (StopReason::VerifierMissing, _) => "verifier_not_available",
        (StopReason::DiagnosticTargetMissing, _) => "repair_target_not_authorized",
        _ => "not_applicable",
    }
}

fn safe_stop_next_user_action(report: &SafeStopReport) -> &'static str {
    match report.stop_reason {
        StopReason::ArtifactCompletionFailed => {
            "complete the missing artifact role or provide a narrower target file"
        }
        StopReason::VerifierMissing => {
            "add a project-local verifier or setup manifest so Anvil can run the requested tests"
        }
        StopReason::VerifierWeak => {
            "provide a verifier command that exercises the requested artifacts"
        }
        StopReason::DiagnosticTargetMissing => {
            "identify the file that should be repaired or narrow the task scope"
        }
        StopReason::RepairExhausted => match report.failure_type {
            VerifierFailureType::AssertionFailure | VerifierFailureType::RepairExhausted => {
                "clarify the expected behavior or provide authoritative examples before retrying"
            }
            _ => "inspect the verifier diagnostics and retry with a narrower repair target",
        },
        StopReason::VerifierFailedSafeStop => match report.failure_type {
            VerifierFailureType::AssertionFailure => {
                "clarify which expected value is authoritative before changing implementation or tests"
            }
            VerifierFailureType::MissingVerifierOrConfig => {
                "add missing verifier or dependency configuration"
            }
            _ => "retry after narrowing the failing behavior or repair target",
        },
    }
}

fn render_safe_stop_payload_trimmed(report: &SafeStopReport, tier: TrimTier) -> serde_json::Value {
    let mut payload = render_safe_stop_payload(report, true);
    let map = payload.as_object_mut().expect("safe_stop_payload object");
    if tier == TrimTier::Tier1 || tier == TrimTier::Tier2 || tier == TrimTier::Tier3 {
        map.insert("output_excerpt".to_string(), serde_json::json!(""));
        map.insert("command".to_string(), serde_json::json!(""));
    }
    if tier == TrimTier::Tier2 || tier == TrimTier::Tier3 {
        map.insert("actual_actions".to_string(), serde_json::json!([]));
        map.insert("owned_test_artifacts".to_string(), serde_json::json!([]));
        map.insert(
            "exhausted_attempts_summary".to_string(),
            serde_json::Value::Null,
        );
    }
    if tier == TrimTier::Tier3 {
        map.insert("expected_target".to_string(), serde_json::Value::Null);
        map.insert(
            "diagnostic_target_missing_reason".to_string(),
            serde_json::Value::Null,
        );
        map.insert("failure_signature".to_string(), serde_json::json!(""));
    }
    payload
}
