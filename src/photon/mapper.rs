use std::path::Path;

use sha2::{Digest, Sha256};

use crate::logging::mask_payload_inplace;
use crate::photon::schema::{ContextPackRequest, EvaluateRequest};
use crate::session::feedback::mask_secrets;

pub const PHOTON_CONTEXT_PACK_SCHEMA_VERSION: &str = "action-memory.v0.2";
pub const PHOTON_EVALUATE_SCHEMA_VERSION: &str = "action-memory.v0.2";
pub const PHOTON_AGENT_NAME: &str = "anvil";
/// Manual sync with store.rs set_active_task literal 240.
pub const MAX_CONTEXT_PACK_TASK_BYTES: usize = 240;
pub const MAX_CONTEXT_PACK_WORKING_MEMORY_BYTES: usize = 4096;
pub const MAX_CONTEXT_PACK_TOOL_ARG_BYTES: usize = 256;
pub const MAX_CONTEXT_PACK_TOOL_NAME_BYTES: usize = 64;
pub const MAX_CONTEXT_PACK_RECENT_TOOLS: usize = 5;
/// Manual sync with store.rs private const MAX_TOUCHED_FILES = 12.
pub const MAX_CONTEXT_PACK_TOUCHED_FILES: usize = 12;

pub struct PhotonGateInputs<'a> {
    pub photon_present: bool,
    pub shadow_mode: bool,
    pub canary: u16,
    pub session_id: &'a str,
    pub turn_idx: usize,
}

pub struct RecentToolCall {
    pub name: String,
    pub args_summary: String,
}

pub struct ContextPackInputs<'a> {
    pub task: Option<&'a str>,
    pub repo_path: &'a Path,
    pub branch: Option<&'a str>,
    pub commit: Option<&'a str>,
    pub working_memory_text: Option<&'a str>,
    pub touched_files: &'a [String],
    pub recent_tool_summary: &'a [RecentToolCall],
    pub selected_case_ids: &'a [String],
    pub selected_anti_pattern_ids: &'a [String],
    pub selected_precaution_ids: &'a [String],
}

/// SHA-256 lower 8 bytes as u64 for deterministic canary sampling.
pub fn deterministic_canary_hash(session_id: &str, turn_idx: usize) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(session_id.as_bytes());
    hasher.update(b"\x00");
    hasher.update(turn_idx.to_le_bytes());
    let digest = hasher.finalize();
    u64::from_le_bytes(digest[..8].try_into().expect("slice is 8 bytes"))
}

pub fn should_send_context_pack(gate: &PhotonGateInputs<'_>) -> bool {
    if !gate.photon_present {
        return false;
    }
    if gate.shadow_mode {
        return true;
    }
    if gate.canary == 0 {
        return false;
    }
    if gate.canary >= 1000 {
        return true;
    }
    deterministic_canary_hash(gate.session_id, gate.turn_idx) % 1000 < gate.canary as u64
}

fn truncate_to_bytes(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn normalize_tool_name(raw: &str) -> String {
    let masked = mask_secrets(raw);
    let capped = truncate_to_bytes(&masked, MAX_CONTEXT_PACK_TOOL_NAME_BYTES);
    let normalized: String = capped
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if normalized.is_empty() {
        "unknown".to_string()
    } else {
        normalized
    }
}

pub fn build_context_pack_request(inputs: &ContextPackInputs<'_>) -> ContextPackRequest {
    let task_text = inputs.task.map(|t| {
        let masked = mask_secrets(t);
        truncate_to_bytes(&masked, MAX_CONTEXT_PACK_TASK_BYTES).to_string()
    });

    let repo_root = inputs
        .repo_path
        .canonicalize()
        .unwrap_or_else(|_| inputs.repo_path.to_path_buf())
        .to_string_lossy()
        .into_owned();
    let repo_root = mask_secrets(&repo_root);
    let repo_name = inputs
        .repo_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());

    let touched_files: Vec<String> = inputs
        .touched_files
        .iter()
        .filter(|f| {
            let p = Path::new(f.as_str());
            // Drop any path containing a `..` component.
            !p.components()
                .any(|c| c == std::path::Component::ParentDir)
                // Drop absolute paths that are not relative to workspace.
                && !p.is_absolute()
        })
        .take(MAX_CONTEXT_PACK_TOUCHED_FILES)
        .map(|f| mask_secrets(f))
        .collect();

    let recent_tools: Vec<serde_json::Value> = inputs
        .recent_tool_summary
        .iter()
        .take(MAX_CONTEXT_PACK_RECENT_TOOLS)
        .map(|tc| {
            let name = normalize_tool_name(&tc.name);
            let masked_args = mask_secrets(&tc.args_summary);
            let args = truncate_to_bytes(&masked_args, MAX_CONTEXT_PACK_TOOL_ARG_BYTES).to_string();
            serde_json::json!({ "name": name, "args_summary": args })
        })
        .collect();

    let request_id = uuid::Uuid::now_v7().to_string();

    let mut value = serde_json::json!({
        "schema_version": PHOTON_CONTEXT_PACK_SCHEMA_VERSION,
        "request_id": request_id,
        "agent": {
            "name": PHOTON_AGENT_NAME,
            "version": env!("CARGO_PKG_VERSION"),
        },
        "repo": {
            "root": repo_root,
            "name": repo_name,
            "branch": inputs.branch,
            "commit": inputs.commit,
        },
        "task": {
            "user_request": task_text,
            "mode": "act",
        },
        "working_memory": {
            "active_task": task_text,
            "touched_files": touched_files,
        },
        // Additional Anvil-specific fields for extended context
        "recent_tool_summary": recent_tools,
        "selected_cases": inputs.selected_case_ids,
        "selected_anti_patterns": inputs.selected_anti_pattern_ids,
        "selected_precautions": inputs.selected_precaution_ids,
    });

    mask_payload_inplace(&mut value);
    ContextPackRequest(value)
}

// ---------------------------------------------------------------------------
// Issue #592: user-explicit feedback evaluate request builder
// ---------------------------------------------------------------------------

/// Inputs gathered by the `photon_user_feedback` adapter when shipping an
/// explicit feedback event (`thumbs-up` / `thumbs-down` / `correct` / `rule`)
/// to the photon sidecar via `/v1/evaluate`. All references are borrowed from
/// the caller; the builder produces an owned `EvaluateRequest`.
pub(crate) struct FeedbackEvaluateInputs<'a> {
    pub session_id: &'a str,
    pub turn_index: usize,
    pub command: &'a str,
    pub summary_ids: &'a [String],
    /// One of the four `OUTCOME_*` literals defined in `photon_user_feedback.rs`
    /// (`user_positive` / `user_negative` / `user_correction` / `user_rule`).
    /// Kept as `&str` so the photon-side enum (`OUTCOME_VALUES`) can evolve
    /// independently of the agent build.
    pub outcome: &'a str,
    pub draft_id: Option<&'a str>,
    pub source_context_pack_request_id: Option<&'a str>,
    pub feedback_event_id: &'a str,
}

/// Build an `EvaluateRequest` carrying a `feedback_event` object. The payload
/// is recursively masked via `mask_payload_inplace` as a final defense-line
/// (DR4-001) before being returned.
///
/// CB-002: A top-level `request_id` is emitted alongside `feedback_event` so
/// the photon sidecar can use it for replay/idempotency on the request side.
/// The `feedback_event.feedback_event_id` remains the application-level
/// dedup key. The two are intentionally distinct: `request_id` is a fresh
/// uuid-v7 per HTTP call (retried request reuses it for idempotency), while
/// `feedback_event_id` is a deterministic hash of `(session_id, turn,
/// command, ts, counter)` per logical event.
pub(crate) fn build_feedback_evaluate_request(
    inputs: &FeedbackEvaluateInputs<'_>,
) -> EvaluateRequest {
    let masked_summary_ids: Vec<String> =
        inputs.summary_ids.iter().map(|s| mask_secrets(s)).collect();
    let request_id = uuid::Uuid::now_v7().to_string();
    let mut value = serde_json::json!({
        "schema_version": PHOTON_EVALUATE_SCHEMA_VERSION,
        "request_id": request_id,
        "agent": {
            "name": PHOTON_AGENT_NAME,
            "version": env!("CARGO_PKG_VERSION"),
        },
        "feedback_event": {
            "feedback_event_id": inputs.feedback_event_id,
            "session_id": inputs.session_id,
            "turn_index": inputs.turn_index,
            "command": inputs.command,
            "outcome": inputs.outcome,
            "summary_ids": masked_summary_ids,
            "draft_id": inputs.draft_id,
            "source_context_pack_request_id": inputs.source_context_pack_request_id,
        },
    });
    mask_payload_inplace(&mut value);
    EvaluateRequest(value)
}

#[cfg(test)]
mod feedback_evaluate_tests {
    use super::*;

    #[test]
    fn build_feedback_evaluate_request_shape() {
        let ids = vec!["seed_a".to_string(), "seed_b".to_string()];
        let inputs = FeedbackEvaluateInputs {
            session_id: "session-test",
            turn_index: 3,
            command: "/photon-thumbs-up",
            summary_ids: &ids,
            outcome: "user_positive",
            draft_id: None,
            source_context_pack_request_id: Some("cp-req-123"),
            feedback_event_id: "fb_event_abc",
        };
        let req = build_feedback_evaluate_request(&inputs);
        let v = req.0;
        assert_eq!(v["schema_version"], PHOTON_EVALUATE_SCHEMA_VERSION);
        assert_eq!(v["agent"]["name"], PHOTON_AGENT_NAME);
        // CB-002: top-level request_id is present, non-empty, and distinct
        // from feedback_event.feedback_event_id (different SSOTs / semantics).
        let request_id = v["request_id"]
            .as_str()
            .expect("top-level request_id must be a string");
        assert!(!request_id.is_empty(), "request_id must be non-empty");
        let fb = &v["feedback_event"];
        let fb_event_id = fb["feedback_event_id"].as_str().expect("string");
        assert_ne!(
            request_id, fb_event_id,
            "request_id and feedback_event_id must be independent identifiers"
        );
        assert_eq!(fb["feedback_event_id"], "fb_event_abc");
        assert_eq!(fb["session_id"], "session-test");
        assert_eq!(fb["turn_index"], 3);
        assert_eq!(fb["command"], "/photon-thumbs-up");
        assert_eq!(fb["outcome"], "user_positive");
        assert_eq!(fb["summary_ids"][0], "seed_a");
        assert_eq!(fb["summary_ids"][1], "seed_b");
        assert!(fb["draft_id"].is_null());
        assert_eq!(fb["source_context_pack_request_id"], "cp-req-123");
    }

    #[test]
    fn build_feedback_evaluate_request_request_id_is_unique_per_call() {
        // CB-002: each call must produce a fresh top-level request_id so the
        // photon sidecar's idempotency cache treats them as distinct HTTP
        // requests. Same call site, identical inputs → different request_ids.
        let ids: Vec<String> = vec![];
        let inputs = FeedbackEvaluateInputs {
            session_id: "s",
            turn_index: 0,
            command: "/photon-thumbs-up",
            summary_ids: &ids,
            outcome: "user_positive",
            draft_id: None,
            source_context_pack_request_id: None,
            feedback_event_id: "fb_event_x",
        };
        let a = build_feedback_evaluate_request(&inputs).0["request_id"]
            .as_str()
            .unwrap()
            .to_string();
        let b = build_feedback_evaluate_request(&inputs).0["request_id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_ne!(a, b, "request_id must be fresh per call");
    }

    #[test]
    fn build_feedback_evaluate_request_masks_secret_in_summary_id() {
        let ids = vec!["token=sk_live_abcd1234efgh5678".to_string()];
        let inputs = FeedbackEvaluateInputs {
            session_id: "s",
            turn_index: 1,
            command: "/photon-thumbs-up",
            summary_ids: &ids,
            outcome: "user_positive",
            draft_id: None,
            source_context_pack_request_id: None,
            feedback_event_id: "fb_event_z",
        };
        let req = build_feedback_evaluate_request(&inputs);
        let raw = serde_json::to_string(&req.0).unwrap();
        assert!(
            !raw.contains("sk_live_abcd1234efgh5678"),
            "raw secret must be masked: {raw}"
        );
    }

    #[test]
    fn build_feedback_evaluate_request_with_draft_id() {
        let ids: Vec<String> = vec![];
        let inputs = FeedbackEvaluateInputs {
            session_id: "s",
            turn_index: 5,
            command: "/photon-correct",
            summary_ids: &ids,
            outcome: "user_correction",
            draft_id: Some("draft_abcdef0123456789"),
            source_context_pack_request_id: None,
            feedback_event_id: "fb_event_q",
        };
        let req = build_feedback_evaluate_request(&inputs);
        assert_eq!(
            req.0["feedback_event"]["draft_id"],
            "draft_abcdef0123456789"
        );
        assert_eq!(req.0["feedback_event"]["outcome"], "user_correction");
    }
}
