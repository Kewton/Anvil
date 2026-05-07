use std::path::Path;

use sha2::{Digest, Sha256};

use crate::logging::mask_payload_inplace;
use crate::photon::schema::ContextPackRequest;
use crate::session::feedback::mask_secrets;

pub const PHOTON_CONTEXT_PACK_SCHEMA_VERSION: u8 = 1;
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
    let task = inputs.task.map(|t| {
        let masked = mask_secrets(t);
        truncate_to_bytes(&masked, MAX_CONTEXT_PACK_TASK_BYTES).to_string()
    });

    let repo_path = inputs
        .repo_path
        .canonicalize()
        .unwrap_or_else(|_| inputs.repo_path.to_path_buf())
        .to_string_lossy()
        .into_owned();
    let repo_path = mask_secrets(&repo_path);

    let working_memory = inputs.working_memory_text.map(|w| {
        let masked = mask_secrets(w);
        truncate_to_bytes(&masked, MAX_CONTEXT_PACK_WORKING_MEMORY_BYTES).to_string()
    });

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

    let mut value = serde_json::json!({
        "schema_version": PHOTON_CONTEXT_PACK_SCHEMA_VERSION,
        "task": task,
        "repo_path": repo_path,
        "branch": inputs.branch,
        "commit": inputs.commit,
        "working_memory": working_memory,
        "touched_files": touched_files,
        "recent_tool_summary": recent_tools,
        "selected_cases": inputs.selected_case_ids,
        "selected_anti_patterns": inputs.selected_anti_pattern_ids,
        "selected_precautions": inputs.selected_precaution_ids,
    });

    mask_payload_inplace(&mut value);
    ContextPackRequest(value)
}
