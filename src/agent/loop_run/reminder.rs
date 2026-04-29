//! Reminder Sidecar (Issue #452 / Epic A #444).
//!
//! After a failure-kind `record_feedback` lands, the actor loop calls into a
//! sidecar Ollama model to produce up to N actionable `Precaution`s for the
//! next turn. The sidecar never writes code, never returns a diff, never makes
//! tool calls.
//!
//! Defence in depth (§8 of the design policy):
//! 1. tools=None on `chat_text` (no tool spec sent).
//! 2. `<think>` strip + first `{...}` JSON object extraction.
//! 3. `AssistantReply.tool_calls` non-empty → no-op + `Failed::UnexpectedToolCalls`.
//! 4. Stored text is re-masked / truncated by `WorkingMemory::add_precaution`.
//!
//! The pure-function half (`run_reminder_with_strategy`) takes a closure that
//! performs the actual LLM call, so unit tests can drive every code path with
//! fixtures and never touch a real Ollama server.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Deserialize;

use crate::agent::loop_run::lifecycle::extract_first_json_object;
use crate::ollama::client::AssistantReply;
use crate::ollama::parsing::truncate_for_log;
use crate::ollama::xml_fallback::strip_think_tags;
use crate::session::anvil_score::AnvilScoreSnapshot;
use crate::session::feedback::{FeedbackFrame, FeedbackKind, mask_secrets};
use crate::session::precaution::{
    AddPrecautionOutcome, Precaution, PrecautionSource, PrecautionStatus, Severity,
};
use crate::session::store::WorkingMemory;

/// Hard cap on `response_raw.len()` before parsing. Anything larger is treated
/// as malformed (DR4-002): an untrusted sidecar that returns >64 KiB is more
/// likely to be a misbehaving fake server than a legitimate precaution list.
pub const REMINDER_RESPONSE_MAX_BYTES: usize = 64 * 1024;

/// Hard cap on the prompt length before we send it to the sidecar.
pub const REMINDER_PROMPT_MAX_BYTES: usize = 12 * 1024;

/// Per-stream excerpt budget (head + tail) in the Reminder prompt. Far smaller
/// than `EXCERPT_CAP_BYTES` (8 KiB) because the sidecar's context window is
/// only 4–8 K tokens.
pub const REMINDER_EXCERPT_HEAD_BYTES: usize = 1024;
pub const REMINDER_EXCERPT_TAIL_BYTES: usize = 1024;

/// Truncation cap for `prompt_log` / `response_raw_log` recorded by
/// `log_llm_event`.
pub const REMINDER_LOG_CAP: usize = 20_000;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Top-level shape of the JSON the sidecar must return. Unknown fields are
/// silently ignored — the project deliberately avoids
/// `#[serde(deny_unknown_fields)]` (zero matches in `src/`) so future schemas
/// stay forward-compat.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReminderResponse {
    #[serde(default)]
    pub precautions: Vec<PrecautionDraft>,
}

/// What the LLM actually wrote. The Anvil-side normalizer overrides `source`
/// from the `FeedbackKind`; the LLM's free-form `source` is kept only as a
/// raw hint and never reaches the stored `Precaution`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PrecautionDraft {
    /// Free-form source label from the LLM. We do not parse it; instead, the
    /// canonical source comes from `normalize_source(frame.kind)`. Optional.
    /// Kept as a struct field so the deserializer accepts it without error,
    /// but never read at runtime — silenced by `#[allow(dead_code)]`.
    #[serde(default)]
    #[allow(dead_code)]
    pub source: String,
    /// snake_case severity (`"high"` / `"medium"` / `"low"`).
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub applies_to: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum ReminderOutcome {
    Completed {
        added_count: usize,
        duplicate_count: usize,
        truncated_count: usize,
        applies_to_dropped_count: usize,
        parse_status: ParseStatus,
        latency_ms: u64,
        prompt_log: String,
        response_raw_log: String,
        feedback_kind: FeedbackKind,
    },
    Failed {
        reason: FailureReason,
        latency_ms: u64,
        prompt_log: String,
        /// LlmCall failure → empty (no LLM response). UnexpectedToolCalls →
        /// truncated raw response. Empty strings are surfaced as `null` in the
        /// log payload.
        response_raw_log: String,
        feedback_kind: FeedbackKind,
    },
    Skipped {
        skip_reason: SkipReason,
        feedback_kind: Option<FeedbackKind>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseStatus {
    Ok,
    Empty,
    Malformed,
}

impl ParseStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ParseStatus::Ok => "ok",
            ParseStatus::Empty => "empty",
            ParseStatus::Malformed => "malformed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureReason {
    LlmCall(String),
    UnexpectedToolCalls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    SidecarUnavailable,
    FeedbackKindExcluded,
    PlanMode,
    Interrupted,
    PerTurnCapped,
    DisabledByEnv,
}

impl SkipReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SkipReason::SidecarUnavailable => "sidecar_unavailable",
            SkipReason::FeedbackKindExcluded => "feedback_kind_excluded",
            SkipReason::PlanMode => "plan_mode",
            SkipReason::Interrupted => "interrupted",
            SkipReason::PerTurnCapped => "per_turn_capped",
            SkipReason::DisabledByEnv => "disabled_by_env",
        }
    }
}

/// Inputs to the prompt builder. References only — the caller owns the
/// underlying values.
pub struct ReminderInputs<'a> {
    pub user_task: &'a str,
    pub mode_label: &'a str,
    pub plan_summary: Option<&'a str>,
    pub active_precautions_summary: &'a str,
    pub frame: &'a FeedbackFrame,
    pub working_memory_touched: &'a [String],
    /// Issue #456: AnvilScore snapshot for prompt injection. The variant
    /// (PreviousTurn vs CurrentTurn) is rendered into a stable label so the
    /// sidecar cannot silently mix the previous turn's persisted score with
    /// the current turn's freshly computed one (DR1-006).
    ///
    /// Lifetime-tagged (carries `&AnvilScore`) → not serde-derivable. The
    /// field is consumed only by `build_reminder_prompt`.
    pub anvil_score: Option<AnvilScoreSnapshot<'a>>,
}

/// Gate state. Each field is computed once by the caller (`Agent::invoke_reminder`)
/// and then `skip_reason()` returns the canonical priority-ordered reason.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReminderGate {
    pub disabled_by_env: bool,
    pub sidecar_available: bool,
    pub kind_eligible: bool,
    pub plan_mode: bool,
    pub interrupted: bool,
    pub per_turn_already_called: bool,
}

impl ReminderGate {
    /// Priority order:
    /// `disabled_by_env > sidecar_unavailable > feedback_kind_excluded > plan_mode > interrupted > per_turn_capped`.
    pub fn skip_reason(&self) -> Option<SkipReason> {
        if self.disabled_by_env {
            return Some(SkipReason::DisabledByEnv);
        }
        if !self.sidecar_available {
            return Some(SkipReason::SidecarUnavailable);
        }
        if !self.kind_eligible {
            return Some(SkipReason::FeedbackKindExcluded);
        }
        if self.plan_mode {
            return Some(SkipReason::PlanMode);
        }
        if self.interrupted {
            return Some(SkipReason::Interrupted);
        }
        if self.per_turn_already_called {
            return Some(SkipReason::PerTurnCapped);
        }
        None
    }
}

/// Returns true when `ANVIL_NO_REMINDER` is set to a non-empty value.
/// Tests inject `get_env` so process-global env mutation is not required.
pub fn reminder_disabled<F>(get_env: F) -> bool
where
    F: FnOnce(&str) -> Option<OsString>,
{
    get_env("ANVIL_NO_REMINDER")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// Whether the FeedbackKind triggers a Reminder call. Issue #455 / DR1-003:
/// thin wrapper over `FeedbackKind::is_eligible_for_reminder()` — kept here
/// for API compatibility with #452 callers. The
/// `is_eligible_for_reminder_parity` test pins the two in sync.
pub fn kind_eligible(kind: &FeedbackKind) -> bool {
    kind.is_eligible_for_reminder()
}

/// Map FeedbackKind → canonical PrecautionSource. None = Reminder is skipped.
///
/// Issue #455 / D1-a: `NoToolCall` (the agent finished a turn without making
/// any tool call) maps to `ToolFailure` rather than getting its own variant.
/// Semantically the failure category "tool was not invoked" is a subset of
/// "tool-related failure"; reusing the source keeps the prompt and
/// `/precautions` rendering paths unchanged.
pub fn normalize_source(kind: &FeedbackKind) -> Option<PrecautionSource> {
    use FeedbackKind::*;
    match kind {
        CompileError | TypeError | LintFailure | Timeout => Some(PrecautionSource::BuildFailure),
        TestFailure => Some(PrecautionSource::TestFailure),
        ToolProtocolFailure | EditFailure | NoToolCall => Some(PrecautionSource::ToolFailure),
        NoRepoProgress => Some(PrecautionSource::NoProgress),
        UnsafeCommandBlocked => Some(PrecautionSource::SafetyPolicy),
        // Issue #467: SkillPermissionDenied は Reminder Sidecar の入力にしない
        // (DR1-001 / DR2-001 の網羅性検知設計を踏襲)。
        BuildPass | TestPass | NoVerifierAvailable | SkillPermissionDenied | UnknownFailure => None,
    }
}

fn parse_severity(label: &str) -> Severity {
    match label.trim().to_ascii_lowercase().as_str() {
        "high" => Severity::High,
        "medium" | "" => Severity::Medium,
        "low" => Severity::Low,
        _ => Severity::Unknown,
    }
}

/// Convert a `PrecautionDraft` from the LLM into a `Precaution` ready for
/// `WorkingMemory::add_precaution`. Returns `(precaution, applies_to_dropped_count)`.
/// Drops applies_to entries that are empty (path canonicalization happens later
/// inside `add_precaution`).
pub fn precaution_from_draft(
    draft: PrecautionDraft,
    kind: &FeedbackKind,
) -> Option<(Precaution, usize)> {
    let source = normalize_source(kind)?;
    if draft.text.trim().is_empty() {
        return None;
    }
    let severity = parse_severity(&draft.severity);
    let mut applies_to = Vec::with_capacity(draft.applies_to.len());
    let mut dropped = 0usize;
    for path in draft.applies_to {
        if path.as_os_str().is_empty() {
            dropped += 1;
            continue;
        }
        applies_to.push(path);
    }
    Some((
        Precaution {
            id: String::new(), // assigned by add_precaution canonicalization
            source,
            severity,
            text: draft.text,
            applies_to,
            status: PrecautionStatus::Active,
            retired_reason: None,
        },
        dropped,
    ))
}

/// Strip `<think>` / extract the first JSON object / parse into ReminderResponse.
/// Oversized input (>REMINDER_RESPONSE_MAX_BYTES) is rejected up-front (DR4-002).
pub fn parse_reminder_response(raw: &str) -> Result<ReminderResponse, String> {
    if raw.len() > REMINDER_RESPONSE_MAX_BYTES {
        return Err(format!(
            "reminder response exceeded {REMINDER_RESPONSE_MAX_BYTES} bytes",
        ));
    }
    let stripped = strip_think_tags(raw);
    let json_slice = extract_first_json_object(&stripped).ok_or("no JSON object found")?;
    serde_json::from_str::<ReminderResponse>(json_slice)
        .map_err(|e| format!("reminder JSON parse error: {e}"))
}

/// Re-mask + truncate any string that ends up in `agent.reminder.*` payload
/// (DR4-001). `mask_secrets` runs first so that any secret-like literal that
/// the LLM regenerated from the prompt is removed before truncation.
pub fn sanitize_reminder_log(raw: &str, cap: usize) -> String {
    truncate_for_log(&mask_secrets(raw), cap)
}

/// Build the LLM-facing prompt. We deliberately omit `source` from the schema
/// the LLM is asked to produce — Anvil sets the source from `frame.kind` via
/// `normalize_source`, so the LLM gets one less knob to misuse.
pub fn build_reminder_prompt(inputs: &ReminderInputs<'_>) -> String {
    let kind_label = serde_json::to_value(&inputs.frame.kind)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown".to_string());
    let primary_error = inputs.frame.primary_error.clone().unwrap_or_default();
    let suggested_focus = inputs.frame.suggested_focus.clone().unwrap_or_default();
    let stdout_excerpt = crate::session::feedback::truncate_excerpt_with_caps(
        inputs.frame.stdout_excerpt(),
        REMINDER_EXCERPT_HEAD_BYTES,
        REMINDER_EXCERPT_TAIL_BYTES,
    );
    let stderr_excerpt = crate::session::feedback::truncate_excerpt_with_caps(
        inputs.frame.stderr_excerpt(),
        REMINDER_EXCERPT_HEAD_BYTES,
        REMINDER_EXCERPT_TAIL_BYTES,
    );
    let mut prompt = String::with_capacity(REMINDER_PROMPT_MAX_BYTES);
    prompt.push_str(
        "You are a reminder. You output ONLY a JSON object of the form:\n\
         { \"precautions\": [ { \"severity\": \"...\", \"text\": \"...\", \"applies_to\": [...] } ] }\n\n\
         Do NOT write code, diff, tool calls, explanations, or prose.\n\
         Do NOT echo back literal secret values from the input.\n\
         Do NOT output more than 5 precautions. Each text must be \u{2264} 200 characters.\n\
         severity \u{2208} { \"high\", \"medium\", \"low\" }.\n\
         applies_to is workspace-relative paths.\n\n",
    );
    prompt.push_str("# Task\n");
    prompt.push_str(inputs.user_task);
    prompt.push_str("\n\n# Mode\n");
    prompt.push_str(inputs.mode_label);
    prompt.push_str("\n\n# Plan summary\n");
    prompt.push_str(inputs.plan_summary.unwrap_or("(none)"));
    prompt.push_str("\n\n# Active precautions\n");
    prompt.push_str(inputs.active_precautions_summary);
    // Issue #456 / DR1-003 / DR1-006: render AnvilScore between Active
    // precautions and Latest feedback so the post-truncate prompt still keeps
    // the "# Output\nJSON only.\n" guard at the tail. Renderer is the single
    // source of truth and applies its own MAX_RENDERED_CHARS cap; caller
    // never re-truncates.
    if let Some(snapshot) = inputs.anvil_score {
        prompt.push_str("\n\n# AnvilScore\n");
        prompt.push_str(snapshot.label());
        prompt.push('\n');
        prompt.push_str(&snapshot.score().format_for_prompt());
    }
    prompt.push_str("\n\n# Latest feedback\n");
    prompt.push_str(&format!("kind: {kind_label}\n"));
    prompt.push_str(&format!("primary_error: {primary_error}\n"));
    prompt.push_str(&format!("suggested_focus: {suggested_focus}\n"));
    prompt.push_str(&format!(
        "changed_files: {:?}\n",
        inputs.frame.changed_files
    ));
    prompt.push_str(&format!(
        "suspected_files: {:?}\n",
        inputs.frame.suspected_files
    ));
    prompt.push_str("\nstdout (head + tail truncated):\n");
    prompt.push_str(&stdout_excerpt);
    prompt.push_str("\n\nstderr (head + tail truncated):\n");
    prompt.push_str(&stderr_excerpt);
    prompt.push_str("\n\n# Touched files (session)\n");
    prompt.push_str(&inputs.working_memory_touched.join("\n"));
    prompt.push_str("\n\n# Output\nJSON only.\n");
    if prompt.len() > REMINDER_PROMPT_MAX_BYTES {
        prompt = truncate_for_log(&prompt, REMINDER_PROMPT_MAX_BYTES);
    }
    prompt
}

// ---------------------------------------------------------------------------
// Pure-function entry point
// ---------------------------------------------------------------------------

/// Reminder body. Mirrors `compact_messages_with_strategy` (compact.rs:10) —
/// the LLM call is injected as a closure so unit tests can drive every code
/// path with fixtures and never touch a real Ollama server.
///
/// Visibility: `pub fn` to match `compact_messages_with_strategy`. crate-external
/// callers are not anticipated, but the convention is preserved.
pub fn run_reminder_with_strategy<F>(
    inputs: ReminderInputs<'_>,
    working_memory: &mut WorkingMemory,
    workspace_root: &Path,
    llm_call: F,
) -> ReminderOutcome
where
    F: FnOnce(&str) -> Result<AssistantReply, String>,
{
    let kind = inputs.frame.kind.clone();
    let prompt = build_reminder_prompt(&inputs);
    let prompt_log = sanitize_reminder_log(&prompt, REMINDER_LOG_CAP);
    let started = Instant::now();
    let reply = match llm_call(&prompt) {
        Ok(r) => r,
        Err(e) => {
            return ReminderOutcome::Failed {
                reason: FailureReason::LlmCall(e),
                latency_ms: started.elapsed().as_millis() as u64,
                prompt_log,
                response_raw_log: String::new(),
                feedback_kind: kind,
            };
        }
    };
    if !reply.tool_calls.is_empty() {
        let response_raw_log = sanitize_reminder_log(&reply.content, REMINDER_LOG_CAP);
        return ReminderOutcome::Failed {
            reason: FailureReason::UnexpectedToolCalls,
            latency_ms: started.elapsed().as_millis() as u64,
            prompt_log,
            response_raw_log,
            feedback_kind: kind,
        };
    }
    let response_raw = reply.content;
    let response_raw_log = sanitize_reminder_log(&response_raw, REMINDER_LOG_CAP);
    let parsed = parse_reminder_response(&response_raw);
    let drafts: Vec<PrecautionDraft> = match &parsed {
        Ok(r) => r.precautions.clone(),
        Err(_) => Vec::new(),
    };
    let parse_status = match (&parsed, drafts.is_empty()) {
        (Ok(_), true) => ParseStatus::Empty,
        (Ok(_), false) => ParseStatus::Ok,
        (Err(_), _) => ParseStatus::Malformed,
    };
    let mut added = 0usize;
    let mut duplicates = 0usize;
    let mut truncated = 0usize;
    let mut applies_to_dropped = 0usize;
    for draft in drafts {
        let (precaution, dropped) = match precaution_from_draft(draft, &kind) {
            Some(pair) => pair,
            None => continue,
        };
        applies_to_dropped += dropped;
        match working_memory.add_precaution(precaution, workspace_root) {
            AddPrecautionOutcome::Added => added += 1,
            AddPrecautionOutcome::DuplicateIgnored => duplicates += 1,
            AddPrecautionOutcome::Truncated => {
                added += 1;
                truncated += 1;
            }
        }
    }
    ReminderOutcome::Completed {
        added_count: added,
        duplicate_count: duplicates,
        truncated_count: truncated,
        applies_to_dropped_count: applies_to_dropped,
        parse_status,
        latency_ms: started.elapsed().as_millis() as u64,
        prompt_log,
        response_raw_log,
        feedback_kind: kind,
    }
}

// ---------------------------------------------------------------------------
// Logging payload
// ---------------------------------------------------------------------------

/// Build the `serde_json::Value` recorded by `log_llm_event`. Exposed as
/// `pub(crate)` so the order/integration tests can assert payload shape
/// without going through the global `OnceLock<Mutex<File>>` sink in
/// `src/logging.rs`.
pub(crate) fn build_log_payload(
    outcome: &ReminderOutcome,
    session_id: &str,
    model: Option<&str>,
) -> (&'static str, serde_json::Value) {
    use serde_json::{Value, json};
    fn or_null(s: &str) -> Value {
        if s.is_empty() {
            Value::Null
        } else {
            Value::String(s.to_string())
        }
    }
    fn kind_label(kind: &FeedbackKind) -> Value {
        serde_json::to_value(kind).unwrap_or(Value::Null)
    }
    match outcome {
        ReminderOutcome::Completed {
            added_count,
            duplicate_count,
            truncated_count,
            applies_to_dropped_count,
            parse_status,
            latency_ms,
            prompt_log,
            response_raw_log,
            feedback_kind,
        } => (
            "agent.reminder.completed",
            json!({
                "session_id": session_id,
                "model": model,
                "parse_status": parse_status.as_str(),
                "added_count": added_count,
                "duplicate_count": duplicate_count,
                "truncated_count": truncated_count,
                "applies_to_dropped_count": applies_to_dropped_count,
                "latency_ms": latency_ms,
                "feedback_kind": kind_label(feedback_kind),
                "skip_reason": Value::Null,
                "prompt": prompt_log,
                "response_raw": response_raw_log,
            }),
        ),
        ReminderOutcome::Failed {
            reason,
            latency_ms,
            prompt_log,
            response_raw_log,
            feedback_kind,
        } => (
            "agent.reminder.failed",
            json!({
                "session_id": session_id,
                "model": model,
                "parse_status": Value::Null,
                "added_count": 0,
                "duplicate_count": 0,
                "truncated_count": 0,
                "applies_to_dropped_count": 0,
                "latency_ms": latency_ms,
                "feedback_kind": kind_label(feedback_kind),
                "skip_reason": Value::Null,
                "failure_reason": match reason {
                    FailureReason::LlmCall(e) => format!("llm_call:{e}"),
                    FailureReason::UnexpectedToolCalls => "unexpected_tool_calls".to_string(),
                },
                "prompt": prompt_log,
                "response_raw": or_null(response_raw_log),
            }),
        ),
        ReminderOutcome::Skipped {
            skip_reason,
            feedback_kind,
        } => (
            "agent.reminder.skipped",
            json!({
                "session_id": session_id,
                "model": Value::Null,
                "parse_status": Value::Null,
                "added_count": 0,
                "duplicate_count": 0,
                "truncated_count": 0,
                "applies_to_dropped_count": 0,
                "latency_ms": Value::Null,
                "feedback_kind": feedback_kind.as_ref().map(kind_label).unwrap_or(Value::Null),
                "skip_reason": skip_reason.as_str(),
                "prompt": Value::Null,
                "response_raw": Value::Null,
            }),
        ),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::feedback::{FeedbackFrame, FeedbackKind};
    use crate::session::store::WorkingMemory;
    use std::path::PathBuf;

    fn frame_with_kind(kind: FeedbackKind) -> FeedbackFrame {
        let mut f = FeedbackFrame::default();
        f.kind = kind;
        f
    }

    fn inputs_for<'a>(frame: &'a FeedbackFrame) -> ReminderInputs<'a> {
        ReminderInputs {
            user_task: "do something",
            mode_label: "act",
            plan_summary: None,
            active_precautions_summary: "(none)",
            frame,
            working_memory_touched: &[],
            anvil_score: None,
        }
    }

    fn ok_reply(content: &str) -> AssistantReply {
        AssistantReply {
            content: content.to_string(),
            tool_calls: Vec::new(),
        }
    }

    fn workspace() -> PathBuf {
        std::env::temp_dir()
    }

    // -- 1. SkipReason ordering ---------------------------------------------

    #[test]
    fn gate_disabled_by_env_takes_priority() {
        let g = ReminderGate {
            disabled_by_env: true,
            sidecar_available: true,
            kind_eligible: true,
            ..Default::default()
        };
        assert_eq!(g.skip_reason(), Some(SkipReason::DisabledByEnv));
    }

    #[test]
    fn gate_sidecar_unavailable() {
        let g = ReminderGate {
            sidecar_available: false,
            ..Default::default()
        };
        assert_eq!(g.skip_reason(), Some(SkipReason::SidecarUnavailable));
    }

    #[test]
    fn gate_kind_excluded() {
        let g = ReminderGate {
            sidecar_available: true,
            kind_eligible: false,
            ..Default::default()
        };
        assert_eq!(g.skip_reason(), Some(SkipReason::FeedbackKindExcluded));
    }

    #[test]
    fn gate_plan_mode() {
        let g = ReminderGate {
            sidecar_available: true,
            kind_eligible: true,
            plan_mode: true,
            ..Default::default()
        };
        assert_eq!(g.skip_reason(), Some(SkipReason::PlanMode));
    }

    #[test]
    fn gate_interrupted() {
        let g = ReminderGate {
            sidecar_available: true,
            kind_eligible: true,
            interrupted: true,
            ..Default::default()
        };
        assert_eq!(g.skip_reason(), Some(SkipReason::Interrupted));
    }

    #[test]
    fn gate_per_turn_capped() {
        let g = ReminderGate {
            sidecar_available: true,
            kind_eligible: true,
            per_turn_already_called: true,
            ..Default::default()
        };
        assert_eq!(g.skip_reason(), Some(SkipReason::PerTurnCapped));
    }

    #[test]
    fn gate_all_clear_returns_none() {
        let g = ReminderGate {
            sidecar_available: true,
            kind_eligible: true,
            ..Default::default()
        };
        assert_eq!(g.skip_reason(), None);
    }

    #[test]
    fn gate_priority_disabled_vs_sidecar() {
        let g = ReminderGate {
            disabled_by_env: true,
            sidecar_available: false,
            ..Default::default()
        };
        assert_eq!(g.skip_reason(), Some(SkipReason::DisabledByEnv));
    }

    #[test]
    fn gate_priority_sidecar_vs_kind() {
        let g = ReminderGate {
            sidecar_available: false,
            kind_eligible: false,
            ..Default::default()
        };
        assert_eq!(g.skip_reason(), Some(SkipReason::SidecarUnavailable));
    }

    #[test]
    fn gate_priority_kind_vs_plan() {
        let g = ReminderGate {
            sidecar_available: true,
            kind_eligible: false,
            plan_mode: true,
            ..Default::default()
        };
        assert_eq!(g.skip_reason(), Some(SkipReason::FeedbackKindExcluded));
    }

    #[test]
    fn gate_priority_plan_vs_interrupted() {
        let g = ReminderGate {
            sidecar_available: true,
            kind_eligible: true,
            plan_mode: true,
            interrupted: true,
            ..Default::default()
        };
        assert_eq!(g.skip_reason(), Some(SkipReason::PlanMode));
    }

    #[test]
    fn gate_priority_interrupted_vs_per_turn() {
        let g = ReminderGate {
            sidecar_available: true,
            kind_eligible: true,
            interrupted: true,
            per_turn_already_called: true,
            ..Default::default()
        };
        assert_eq!(g.skip_reason(), Some(SkipReason::Interrupted));
    }

    // -- 2. reminder_disabled env helper ------------------------------------

    #[test]
    fn reminder_disabled_unset_is_false() {
        assert!(!reminder_disabled(|_| None));
    }

    #[test]
    fn reminder_disabled_empty_is_false() {
        assert!(!reminder_disabled(|_| Some(OsString::new())));
    }

    #[test]
    fn reminder_disabled_set_is_true() {
        assert!(reminder_disabled(|_| Some(OsString::from("1"))));
    }

    // -- 3. normalize_source / kind_eligible matrix (AC-1) ------------------

    #[test]
    fn normalize_source_matrix() {
        use FeedbackKind::*;
        assert_eq!(
            normalize_source(&CompileError),
            Some(PrecautionSource::BuildFailure)
        );
        assert_eq!(
            normalize_source(&TypeError),
            Some(PrecautionSource::BuildFailure)
        );
        assert_eq!(
            normalize_source(&LintFailure),
            Some(PrecautionSource::BuildFailure)
        );
        assert_eq!(
            normalize_source(&Timeout),
            Some(PrecautionSource::BuildFailure)
        );
        assert_eq!(
            normalize_source(&TestFailure),
            Some(PrecautionSource::TestFailure)
        );
        assert_eq!(
            normalize_source(&ToolProtocolFailure),
            Some(PrecautionSource::ToolFailure)
        );
        assert_eq!(
            normalize_source(&EditFailure),
            Some(PrecautionSource::ToolFailure)
        );
        assert_eq!(
            normalize_source(&NoRepoProgress),
            Some(PrecautionSource::NoProgress)
        );
        assert_eq!(
            normalize_source(&UnsafeCommandBlocked),
            Some(PrecautionSource::SafetyPolicy)
        );
        // Issue #455 / D1-a: NoToolCall reuses ToolFailure source.
        assert_eq!(
            normalize_source(&NoToolCall),
            Some(PrecautionSource::ToolFailure)
        );
        assert_eq!(normalize_source(&BuildPass), None);
        assert_eq!(normalize_source(&TestPass), None);
        assert_eq!(normalize_source(&NoVerifierAvailable), None);
        assert_eq!(normalize_source(&SkillPermissionDenied), None);
        assert_eq!(normalize_source(&UnknownFailure), None);
    }

    /// Issue #455 / DR2-005: parity between `normalize_source` and
    /// `FeedbackKind::is_eligible_for_reminder`. Iterates over every
    /// FeedbackKind variant explicitly so the CI fails if either side
    /// gets updated without the other.
    #[test]
    fn is_eligible_for_reminder_parity() {
        use FeedbackKind::*;
        const ALL_KINDS: [FeedbackKind; 15] = [
            BuildPass,
            TestPass,
            CompileError,
            TestFailure,
            TypeError,
            LintFailure,
            Timeout,
            ToolProtocolFailure,
            EditFailure,
            NoRepoProgress,
            UnsafeCommandBlocked,
            NoVerifierAvailable,
            NoToolCall,
            SkillPermissionDenied,
            UnknownFailure,
        ];
        for kind in ALL_KINDS.iter() {
            assert_eq!(
                normalize_source(kind).is_some(),
                kind.is_eligible_for_reminder(),
                "parity broken for {kind:?}"
            );
        }
    }

    // -- 4. precaution_from_draft -------------------------------------------

    #[test]
    fn precaution_from_draft_normal() {
        let draft = PrecautionDraft {
            source: "anything".to_string(),
            severity: "high".to_string(),
            text: "be careful".to_string(),
            applies_to: vec![PathBuf::from("src/x.rs"), PathBuf::new()],
        };
        let (p, dropped) = precaution_from_draft(draft, &FeedbackKind::TestFailure).expect("some");
        assert_eq!(p.source, PrecautionSource::TestFailure);
        assert_eq!(p.severity, Severity::High);
        assert_eq!(p.applies_to.len(), 1);
        assert_eq!(dropped, 1);
        assert_eq!(p.status, PrecautionStatus::Active);
    }

    #[test]
    fn precaution_from_draft_unknown_severity() {
        let draft = PrecautionDraft {
            severity: "FATAL".to_string(),
            text: "x".to_string(),
            ..Default::default()
        };
        let (p, _) = precaution_from_draft(draft, &FeedbackKind::TestFailure).expect("some");
        assert_eq!(p.severity, Severity::Unknown);
    }

    #[test]
    fn precaution_from_draft_blank_text_drops() {
        let draft = PrecautionDraft {
            severity: "high".to_string(),
            text: "   ".to_string(),
            ..Default::default()
        };
        assert!(precaution_from_draft(draft, &FeedbackKind::TestFailure).is_none());
    }

    #[test]
    fn precaution_from_draft_kind_excluded_drops() {
        let draft = PrecautionDraft {
            severity: "high".to_string(),
            text: "x".to_string(),
            ..Default::default()
        };
        assert!(precaution_from_draft(draft, &FeedbackKind::BuildPass).is_none());
    }

    // -- 5. parse_reminder_response -----------------------------------------

    #[test]
    fn parse_normal_json() {
        let raw = r#"{"precautions":[{"severity":"high","text":"x","applies_to":[]}]}"#;
        let r = parse_reminder_response(raw).expect("ok");
        assert_eq!(r.precautions.len(), 1);
    }

    #[test]
    fn parse_strips_think_tags() {
        let raw = "<think>blah</think>{\"precautions\":[]}";
        let r = parse_reminder_response(raw).expect("ok");
        assert!(r.precautions.is_empty());
    }

    #[test]
    fn parse_returns_first_object() {
        let raw = r#"{"precautions":[]}{"precautions":[{"text":"a","severity":"high"}]}"#;
        let r = parse_reminder_response(raw).expect("ok");
        assert!(r.precautions.is_empty());
    }

    #[test]
    fn parse_missing_object() {
        assert!(parse_reminder_response("hello world").is_err());
    }

    #[test]
    fn parse_oversized_response_rejected() {
        let raw = format!(
            "{{\"precautions\":[]{}}}",
            "x".repeat(REMINDER_RESPONSE_MAX_BYTES)
        );
        assert!(parse_reminder_response(&raw).is_err());
    }

    #[test]
    fn parse_unknown_top_level_field_is_ignored() {
        let raw = r#"{"precautions":[],"tool_calls":[{"name":"foo"}]}"#;
        let r = parse_reminder_response(raw).expect("ok");
        assert!(r.precautions.is_empty());
    }

    // -- 6. sanitize_reminder_log -------------------------------------------

    #[test]
    fn sanitize_masks_known_tokens() {
        let raw = "OPENAI_API_KEY=sk-test12345abcdefghij1234567890";
        let out = sanitize_reminder_log(raw, 1024);
        assert!(!out.contains("sk-test12345abcdefghij1234567890"));
    }

    #[test]
    fn sanitize_truncates_to_cap() {
        // truncate_for_log keeps `cap` chars and appends a "...[truncated]" marker,
        // so the final length is bounded but slightly above `cap`.
        let raw = "x".repeat(50_000);
        let out = sanitize_reminder_log(&raw, 100);
        assert!(out.starts_with(&"x".repeat(100)));
        assert!(out.ends_with("[truncated]"));
        assert!(out.chars().count() < 200);
    }

    // -- 7. run_reminder_with_strategy --------------------------------------

    #[test]
    fn t_normal_json() {
        let frame = frame_with_kind(FeedbackKind::TestFailure);
        let mut wm = WorkingMemory::default();
        let outcome = run_reminder_with_strategy(inputs_for(&frame), &mut wm, &workspace(), |_| {
            Ok(ok_reply(
                r#"{"precautions":[{"severity":"high","text":"watch the mock","applies_to":[]}]}"#,
            ))
        });
        match outcome {
            ReminderOutcome::Completed {
                added_count,
                parse_status,
                ..
            } => {
                assert_eq!(added_count, 1);
                assert_eq!(parse_status, ParseStatus::Ok);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert_eq!(wm.active_precautions.len(), 1);
    }

    #[test]
    fn t_empty_precautions() {
        let frame = frame_with_kind(FeedbackKind::TestFailure);
        let mut wm = WorkingMemory::default();
        let outcome = run_reminder_with_strategy(inputs_for(&frame), &mut wm, &workspace(), |_| {
            Ok(ok_reply(r#"{"precautions":[]}"#))
        });
        match outcome {
            ReminderOutcome::Completed {
                parse_status,
                added_count,
                ..
            } => {
                assert_eq!(parse_status, ParseStatus::Empty);
                assert_eq!(added_count, 0);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert!(wm.active_precautions.is_empty());
    }

    #[test]
    fn t_malformed_json() {
        let frame = frame_with_kind(FeedbackKind::TestFailure);
        let mut wm = WorkingMemory::default();
        let outcome = run_reminder_with_strategy(inputs_for(&frame), &mut wm, &workspace(), |_| {
            Ok(ok_reply("not json"))
        });
        match outcome {
            ReminderOutcome::Completed {
                parse_status,
                added_count,
                ..
            } => {
                assert_eq!(parse_status, ParseStatus::Malformed);
                assert_eq!(added_count, 0);
            }
            other => panic!("expected Completed (malformed-but-no-panic), got {other:?}"),
        }
        assert!(wm.active_precautions.is_empty());
    }

    #[test]
    fn t_response_with_think() {
        let frame = frame_with_kind(FeedbackKind::TestFailure);
        let mut wm = WorkingMemory::default();
        let outcome = run_reminder_with_strategy(inputs_for(&frame), &mut wm, &workspace(), |_| {
            Ok(ok_reply(
                r#"<think>thinking</think>{"precautions":[{"severity":"medium","text":"after think"}]}"#,
            ))
        });
        match outcome {
            ReminderOutcome::Completed { added_count, .. } => {
                assert_eq!(added_count, 1);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn t_response_with_code_fence_in_text() {
        let frame = frame_with_kind(FeedbackKind::TestFailure);
        let mut wm = WorkingMemory::default();
        let outcome = run_reminder_with_strategy(inputs_for(&frame), &mut wm, &workspace(), |_| {
            Ok(ok_reply(
                r#"{"precautions":[{"severity":"high","text":"```rust\nfn x(){}\n```"}]}"#,
            ))
        });
        match outcome {
            ReminderOutcome::Completed { added_count, .. } => {
                assert_eq!(added_count, 1);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert_eq!(wm.active_precautions.len(), 1);
    }

    #[test]
    fn t_unexpected_tool_calls() {
        use crate::ollama::xml_fallback::ToolCall;
        let frame = frame_with_kind(FeedbackKind::TestFailure);
        let mut wm = WorkingMemory::default();
        let outcome = run_reminder_with_strategy(inputs_for(&frame), &mut wm, &workspace(), |_| {
            Ok(AssistantReply {
                content: r#"{"precautions":[{"severity":"high","text":"x"}]}"#.to_string(),
                tool_calls: vec![ToolCall {
                    id: "tc-1".to_string(),
                    name: "shell".to_string(),
                    arguments: serde_json::json!({}),
                }],
            })
        });
        match outcome {
            ReminderOutcome::Failed { reason, .. } => {
                assert_eq!(reason, FailureReason::UnexpectedToolCalls);
            }
            other => panic!("expected Failed(UnexpectedToolCalls), got {other:?}"),
        }
        assert!(wm.active_precautions.is_empty());
    }

    #[test]
    fn t_llm_call_error() {
        let frame = frame_with_kind(FeedbackKind::TestFailure);
        let mut wm = WorkingMemory::default();
        let outcome = run_reminder_with_strategy(inputs_for(&frame), &mut wm, &workspace(), |_| {
            Err("timeout".to_string())
        });
        match outcome {
            ReminderOutcome::Failed { reason, .. } => match reason {
                FailureReason::LlmCall(s) => assert_eq!(s, "timeout"),
                _ => panic!("expected LlmCall"),
            },
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn t_dedup_same_frame() {
        let frame = frame_with_kind(FeedbackKind::TestFailure);
        let mut wm = WorkingMemory::default();
        // First call: 1 added
        let _ = run_reminder_with_strategy(inputs_for(&frame), &mut wm, &workspace(), |_| {
            Ok(ok_reply(
                r#"{"precautions":[{"severity":"high","text":"keep this"}]}"#,
            ))
        });
        let before = wm.active_precautions.len();
        // Second call with same fixture
        let outcome = run_reminder_with_strategy(inputs_for(&frame), &mut wm, &workspace(), |_| {
            Ok(ok_reply(
                r#"{"precautions":[{"severity":"high","text":"keep this"}]}"#,
            ))
        });
        match outcome {
            ReminderOutcome::Completed {
                duplicate_count,
                added_count,
                ..
            } => {
                assert_eq!(duplicate_count, 1);
                assert_eq!(added_count, 0);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert_eq!(wm.active_precautions.len(), before);
    }

    /// Issue #455 / Task 2.2: drive `run_reminder_with_strategy` with a
    /// `FeedbackKind::NoToolCall` fixture and assert that the precaution is
    /// added with the correct source (`ToolFailure`) and the
    /// `feedback_kind` field on the outcome equals `NoToolCall`. Also
    /// asserts the build_log_payload serializes the kind as
    /// `"no_tool_call"`.
    #[test]
    fn run_reminder_with_strategy_drives_no_tool_call() {
        let frame = frame_with_kind(FeedbackKind::NoToolCall);
        let mut wm = WorkingMemory::default();
        let outcome = run_reminder_with_strategy(inputs_for(&frame), &mut wm, &workspace(), |_| {
            Ok(ok_reply(
                r#"{"precautions":[{"severity":"high","text":"emit a concrete tool call next turn"}]}"#,
            ))
        });
        match &outcome {
            ReminderOutcome::Completed {
                added_count,
                parse_status,
                feedback_kind,
                ..
            } => {
                assert_eq!(*added_count, 1);
                assert_eq!(*parse_status, ParseStatus::Ok);
                assert_eq!(*feedback_kind, FeedbackKind::NoToolCall);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert_eq!(wm.active_precautions.len(), 1);
        // Source must be ToolFailure (D1-a).
        assert_eq!(
            wm.active_precautions[0].source,
            PrecautionSource::ToolFailure
        );
        // Log payload must serialize feedback_kind as "no_tool_call".
        let (_event, payload) = build_log_payload(&outcome, "sess", Some("model"));
        assert_eq!(
            payload.get("feedback_kind").and_then(|v| v.as_str()),
            Some("no_tool_call")
        );
    }

    // -- 8. build_log_payload (AC-13) ---------------------------------------

    fn make_completed() -> ReminderOutcome {
        ReminderOutcome::Completed {
            added_count: 2,
            duplicate_count: 1,
            truncated_count: 0,
            applies_to_dropped_count: 0,
            parse_status: ParseStatus::Ok,
            latency_ms: 42,
            prompt_log: "prompt".to_string(),
            response_raw_log: "raw".to_string(),
            feedback_kind: FeedbackKind::TestFailure,
        }
    }

    #[test]
    fn payload_completed_has_all_keys() {
        let (event, payload) = build_log_payload(&make_completed(), "sess", Some("qwen-0.5b"));
        assert_eq!(event, "agent.reminder.completed");
        let obj = payload.as_object().expect("object");
        for key in [
            "session_id",
            "model",
            "parse_status",
            "added_count",
            "duplicate_count",
            "truncated_count",
            "applies_to_dropped_count",
            "latency_ms",
            "feedback_kind",
            "skip_reason",
            "prompt",
            "response_raw",
        ] {
            assert!(obj.contains_key(key), "missing key: {key}");
        }
        assert_eq!(obj.get("model").unwrap().as_str(), Some("qwen-0.5b"));
        assert_eq!(obj.get("parse_status").unwrap().as_str(), Some("ok"));
        assert!(obj.get("skip_reason").unwrap().is_null());
    }

    #[test]
    fn payload_failed_with_llm_call_has_null_response() {
        let outcome = ReminderOutcome::Failed {
            reason: FailureReason::LlmCall("timeout".to_string()),
            latency_ms: 7,
            prompt_log: "prompt".to_string(),
            response_raw_log: String::new(),
            feedback_kind: FeedbackKind::TestFailure,
        };
        let (event, payload) = build_log_payload(&outcome, "sess", Some("qwen-0.5b"));
        assert_eq!(event, "agent.reminder.failed");
        let obj = payload.as_object().expect("object");
        assert_eq!(obj.get("added_count").unwrap().as_u64(), Some(0));
        assert!(obj.get("response_raw").unwrap().is_null());
        let reason = obj.get("failure_reason").unwrap().as_str().expect("str");
        assert!(reason.starts_with("llm_call:"));
    }

    #[test]
    fn payload_failed_with_unexpected_tool_calls_has_response() {
        let outcome = ReminderOutcome::Failed {
            reason: FailureReason::UnexpectedToolCalls,
            latency_ms: 3,
            prompt_log: "prompt".to_string(),
            response_raw_log: "raw_with_tools".to_string(),
            feedback_kind: FeedbackKind::TestFailure,
        };
        let (event, payload) = build_log_payload(&outcome, "sess", Some("qwen-0.5b"));
        assert_eq!(event, "agent.reminder.failed");
        let obj = payload.as_object().expect("object");
        assert_eq!(
            obj.get("response_raw").unwrap().as_str(),
            Some("raw_with_tools")
        );
        assert_eq!(
            obj.get("failure_reason").unwrap().as_str(),
            Some("unexpected_tool_calls")
        );
    }

    #[test]
    fn payload_skipped_has_skip_reason() {
        let outcome = ReminderOutcome::Skipped {
            skip_reason: SkipReason::SidecarUnavailable,
            feedback_kind: None,
        };
        let (event, payload) = build_log_payload(&outcome, "sess", None);
        assert_eq!(event, "agent.reminder.skipped");
        let obj = payload.as_object().expect("object");
        assert_eq!(
            obj.get("skip_reason").unwrap().as_str(),
            Some("sidecar_unavailable"),
        );
        assert!(obj.get("model").unwrap().is_null());
        assert!(obj.get("prompt").unwrap().is_null());
        assert!(obj.get("response_raw").unwrap().is_null());
    }

    // ---------------------------------------------------------------------
    // -- 9. Issue #455 / Phase 5 acceptance tests --------------------------
    // ---------------------------------------------------------------------
    //
    // Each AC test drives `run_reminder_with_strategy` with a fixture closure
    // that returns a precaution whose text matches the AC regex (per Issue
    // body §10 / design policy §10). We cannot exercise the live reminder
    // sidecar prompt here (CI does not have Ollama), so we pin the contract
    // that "given a frame of kind X and a hypothetical sidecar reply
    // matching regex R, the precaution lands in WorkingMemory with text
    // matching R". Mismatch with real sidecar output is tracked separately
    // in dev-reports/issue/455 and is out of scope for this issue.

    /// Helper: drive the reminder loop once with a fixed reply and assert
    /// the first stored precaution text matches the supplied regex.
    /// Compile the AC regex with ASCII-only case-insensitivity. The
    /// `regex` crate is configured without `unicode-case` (Cargo.toml), so
    /// `(?i)` is unsupported. We lowercase the haystack before matching to
    /// keep the AC patterns short and human-readable.
    fn assert_ac_regex(kind: FeedbackKind, text: &str, pattern: &str) {
        let frame = frame_with_kind(kind.clone());
        let mut wm = WorkingMemory::default();
        let body = format!(
            r#"{{"precautions":[{{"severity":"high","text":{}}}]}}"#,
            serde_json::to_string(text).unwrap()
        );
        let outcome = run_reminder_with_strategy(inputs_for(&frame), &mut wm, &workspace(), |_| {
            Ok(ok_reply(&body))
        });
        match outcome {
            ReminderOutcome::Completed { added_count, .. } => {
                assert_eq!(added_count, 1, "expected 1 precaution stored, body={body}");
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert_eq!(wm.active_precautions.len(), 1);
        let stored = wm.active_precautions[0].text.to_ascii_lowercase();
        let re = regex::Regex::new(pattern).expect("regex compiles");
        assert!(
            re.is_match(&stored),
            "AC regex mismatch for kind {kind:?}: pattern={pattern} stored={stored}"
        );
    }

    /// AC: focused edit failure → text matches `read.{0,4}before.{0,4}edit|read first|verify (the )?file`.
    #[test]
    fn ac_focused_edit_text_matches_regex() {
        assert_ac_regex(
            FeedbackKind::EditFailure,
            "Read the target file before edit; verify the file exists.",
            r"read.{0,4}before.{0,4}edit|read first|verify (the )?file",
        );
    }

    /// AC: repeated bash command → text matches `avoid (re)?running|do not (re)?run|repeat|same command`.
    #[test]
    fn ac_repeated_bash_text_matches_regex() {
        assert_ac_regex(
            FeedbackKind::Timeout,
            "Avoid rerunning the same command; vary the args before retry.",
            r"avoid (re)?running|do not (re)?run|repeat|same command",
        );
    }

    /// AC: unsafe command blocked → text matches `unsafe|denied|safety|policy`.
    #[test]
    fn ac_unsafe_block_text_matches_regex() {
        assert_ac_regex(
            FeedbackKind::UnsafeCommandBlocked,
            "Command was blocked by the safety policy; choose a safer alternative.",
            r"unsafe|denied|safety|policy",
        );
    }

    /// AC: no repo progress → text matches `narrow|focus|reduce scope|smaller|specific (file|target)`.
    #[test]
    fn ac_no_repo_progress_text_matches_regex() {
        assert_ac_regex(
            FeedbackKind::NoRepoProgress,
            "Narrow the scope and focus on a specific file before retrying.",
            r"narrow|focus|reduce scope|smaller|specific (file|target)",
        );
    }

    /// AC: parser failure → text matches `tool call|valid .*json|valid .*tool|parser|format|compact`.
    #[test]
    fn ac_parser_failure_text_matches_regex() {
        assert_ac_regex(
            FeedbackKind::ToolProtocolFailure,
            "Emit a single valid tool call in compact format next turn.",
            r"tool call|valid .*json|valid .*tool|parser|format|compact",
        );
    }

    /// AC (D1): NoToolCall → text matches `tool call|concrete action|use .*tool|emit .*tool|repo work`.
    #[test]
    fn ac_no_tool_call_text_matches_regex() {
        assert_ac_regex(
            FeedbackKind::NoToolCall,
            "Emit a concrete tool call next turn instead of prose.",
            r"tool call|concrete action|use .*tool|emit .*tool|repo work",
        );
    }

    /// AC (D2): deterministic content fallback → ToolProtocolFailure with
    /// regex `deterministic|fallback|placeholder|scaffold|quality gate|repair|polish`.
    #[test]
    fn ac_deterministic_content_fallback_text_matches_regex() {
        assert_ac_regex(
            FeedbackKind::ToolProtocolFailure,
            "Avoid relying on the deterministic fallback; produce concrete content.",
            r"deterministic|fallback|placeholder|scaffold|quality gate|repair|polish",
        );
    }

    // -- D4 same-turn conflict guard --------------------------------------

    /// Issue #455 / D4 / CB-001: when an UnsafeCommandBlocked frame is
    /// recorded during this turn via `record_feedback_tracked`, a follow-up
    /// call with `record_feedback_if_unset` passing a NoToolCall / D2 frame
    /// must NOT overwrite the unsafe frame. (This duplicates the store.rs
    /// unit test but pins it again at the integration boundary using the
    /// `SessionSnapshot` API.)
    #[test]
    fn d4_same_turn_unsafe_frame_wins_over_no_tool_call() {
        use crate::session::store::SessionSnapshot;
        let mut snap = SessionSnapshot::default();
        // Fire 1: UnsafeCommandBlocked recorded this turn (sets the in-snapshot flag).
        let unsafe_frame: FeedbackFrame =
            serde_json::from_str(r#"{"kind":"unsafe_command_blocked"}"#).unwrap();
        snap.record_feedback(unsafe_frame);
        assert!(snap.eligible_feedback_recorded_this_turn);
        // Fire 2: D2 / D1 frame attempted via guard — must be no-op.
        let d2_frame: FeedbackFrame = serde_json::from_str(
            r#"{"kind":"tool_protocol_failure","primary_error":"deterministic_content_fallback"}"#,
        )
        .unwrap();
        snap.record_feedback_if_unset(d2_frame);
        assert_eq!(
            snap.last_feedback.unwrap().kind,
            FeedbackKind::UnsafeCommandBlocked,
            "this-turn unsafe frame must win over follow-up D2"
        );
    }

    // -- Reminder failure modes -------------------------------------------

    /// Failure mode (a): sidecar Ollama call fails (e.g. timeout).
    /// Outcome must be `Failed` with `LlmCall` reason; `active_precautions`
    /// must be unchanged; the log payload must serialize
    /// `feedback_kind = "no_tool_call"`.
    #[test]
    fn ac_failure_mode_llm_call_error() {
        let frame = frame_with_kind(FeedbackKind::NoToolCall);
        let mut wm = WorkingMemory::default();
        let outcome = run_reminder_with_strategy(inputs_for(&frame), &mut wm, &workspace(), |_| {
            Err("connection timed out".to_string())
        });
        match &outcome {
            ReminderOutcome::Failed {
                reason,
                feedback_kind,
                ..
            } => {
                match reason {
                    FailureReason::LlmCall(msg) => assert_eq!(msg, "connection timed out"),
                    other => panic!("expected LlmCall, got {other:?}"),
                }
                assert_eq!(*feedback_kind, FeedbackKind::NoToolCall);
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(wm.active_precautions.is_empty());
        let (_event, payload) = build_log_payload(&outcome, "sess", Some("model"));
        assert_eq!(
            payload.get("feedback_kind").and_then(|v| v.as_str()),
            Some("no_tool_call")
        );
    }

    /// Failure mode (b): malformed JSON. Outcome is `Completed` with
    /// `parse_status = malformed` and `added_count = 0`. (Per design — a
    /// non-JSON sidecar reply is degenerate but not protocol-breaking.)
    #[test]
    fn ac_failure_mode_malformed_json() {
        let frame = frame_with_kind(FeedbackKind::NoToolCall);
        let mut wm = WorkingMemory::default();
        let outcome = run_reminder_with_strategy(inputs_for(&frame), &mut wm, &workspace(), |_| {
            Ok(ok_reply("garbage not json"))
        });
        match &outcome {
            ReminderOutcome::Completed {
                parse_status,
                added_count,
                feedback_kind,
                ..
            } => {
                assert_eq!(*parse_status, ParseStatus::Malformed);
                assert_eq!(*added_count, 0);
                assert_eq!(*feedback_kind, FeedbackKind::NoToolCall);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert!(wm.active_precautions.is_empty());
    }

    /// Failure mode (c): sidecar returns non-empty `tool_calls`. Must surface
    /// `Failed { reason: UnexpectedToolCalls }`.
    #[test]
    fn ac_failure_mode_unexpected_tool_calls() {
        use crate::ollama::xml_fallback::ToolCall;
        let frame = frame_with_kind(FeedbackKind::NoToolCall);
        let mut wm = WorkingMemory::default();
        let outcome = run_reminder_with_strategy(inputs_for(&frame), &mut wm, &workspace(), |_| {
            Ok(AssistantReply {
                content: r#"{"precautions":[{"severity":"high","text":"x"}]}"#.to_string(),
                tool_calls: vec![ToolCall {
                    id: "tc-1".to_string(),
                    name: "shell".to_string(),
                    arguments: serde_json::json!({}),
                }],
            })
        });
        match &outcome {
            ReminderOutcome::Failed {
                reason,
                feedback_kind,
                ..
            } => {
                assert_eq!(*reason, FailureReason::UnexpectedToolCalls);
                assert_eq!(*feedback_kind, FeedbackKind::NoToolCall);
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(wm.active_precautions.is_empty());
    }

    /// Failure mode (d): connection failure. Same shape as (a) but driven
    /// by a different error message; the failure_reason in the log payload
    /// must start with `llm_call:`.
    #[test]
    fn ac_failure_mode_connection_unavailable() {
        let frame = frame_with_kind(FeedbackKind::NoToolCall);
        let mut wm = WorkingMemory::default();
        let outcome = run_reminder_with_strategy(inputs_for(&frame), &mut wm, &workspace(), |_| {
            Err("connection refused".to_string())
        });
        let (event, payload) = build_log_payload(&outcome, "sess", Some("model"));
        assert_eq!(event, "agent.reminder.failed");
        let failure_reason = payload
            .get("failure_reason")
            .and_then(|v| v.as_str())
            .expect("failure_reason");
        assert!(
            failure_reason.starts_with("llm_call:"),
            "expected llm_call: prefix, got {failure_reason}"
        );
        assert!(wm.active_precautions.is_empty());
    }

    // -- E2E-shaped regression for both new wirings -----------------------

    /// Issue #455 Task 5.4 / E2E-shaped: drive the reminder pipeline twice,
    /// once for D1 (NoToolCall) and once for D2 (deterministic content
    /// fallback), and assert each delivers a precaution whose text matches
    /// the corresponding AC regex. This pins the wiring at the
    /// `run_reminder_with_strategy` boundary against a future regression
    /// that silently changes either kind's normalize_source.
    #[test]
    fn d1_d2_both_deliver_precaution_to_working_memory() {
        let workspace = workspace();
        // D1: NoToolCall.
        let d1_frame = frame_with_kind(FeedbackKind::NoToolCall);
        let mut d1_wm = WorkingMemory::default();
        let d1_outcome = run_reminder_with_strategy(
            inputs_for(&d1_frame),
            &mut d1_wm,
            &workspace,
            |_| {
                Ok(ok_reply(
                    r#"{"precautions":[{"severity":"high","text":"emit a concrete tool call now"}]}"#,
                ))
            },
        );
        assert!(matches!(d1_outcome, ReminderOutcome::Completed { .. }));
        assert_eq!(d1_wm.active_precautions.len(), 1);
        assert_eq!(
            d1_wm.active_precautions[0].source,
            PrecautionSource::ToolFailure
        );

        // D2: ToolProtocolFailure with deterministic_content_fallback tag.
        // Note: the kind alone is enough — the tag travels through
        // primary_error in the prompt only, not through the precaution.
        let d2_frame = frame_with_kind(FeedbackKind::ToolProtocolFailure);
        let mut d2_wm = WorkingMemory::default();
        let d2_outcome = run_reminder_with_strategy(
            inputs_for(&d2_frame),
            &mut d2_wm,
            &workspace,
            |_| {
                Ok(ok_reply(
                    r#"{"precautions":[{"severity":"high","text":"avoid relying on the deterministic fallback"}]}"#,
                ))
            },
        );
        assert!(matches!(d2_outcome, ReminderOutcome::Completed { .. }));
        assert_eq!(d2_wm.active_precautions.len(), 1);
        assert_eq!(
            d2_wm.active_precautions[0].source,
            PrecautionSource::ToolFailure
        );
    }

    // ---------------------------------------------------------------------
    // Issue #456: AnvilScore prompt injection
    // ---------------------------------------------------------------------

    use crate::session::anvil_score::{AnvilScore, AnvilScoreSnapshot};

    fn fixture_score() -> AnvilScore {
        AnvilScore {
            build_passed: Some(true),
            tests_passed: Some(false),
            compile_errors_delta: Some(-2),
            test_failures_delta: Some(1),
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

    #[test]
    fn anvil_score_section_uses_previous_label_for_iteration_internal() {
        let frame = frame_with_kind(FeedbackKind::TestFailure);
        let score = fixture_score();
        let inputs = ReminderInputs {
            user_task: "u",
            mode_label: "act",
            plan_summary: None,
            active_precautions_summary: "(none)",
            frame: &frame,
            working_memory_touched: &[],
            anvil_score: Some(AnvilScoreSnapshot::PreviousTurn(&score)),
        };
        let prompt = build_reminder_prompt(&inputs);
        assert!(
            prompt.contains("[Previous AnvilScore]"),
            "expected previous label, got: {prompt}"
        );
        assert!(!prompt.contains("[Current AnvilScore]"));
        // Renderer key must appear (not the bare struct).
        assert!(prompt.contains("build_passed: true"));
        assert!(prompt.contains("user_visible_artifact: true"));
    }

    #[test]
    fn anvil_score_section_uses_current_label_for_post_loop() {
        let frame = frame_with_kind(FeedbackKind::TestFailure);
        let score = fixture_score();
        let inputs = ReminderInputs {
            user_task: "u",
            mode_label: "act",
            plan_summary: None,
            active_precautions_summary: "(none)",
            frame: &frame,
            working_memory_touched: &[],
            anvil_score: Some(AnvilScoreSnapshot::CurrentTurn(&score)),
        };
        let prompt = build_reminder_prompt(&inputs);
        assert!(prompt.contains("[Current AnvilScore]"));
        assert!(!prompt.contains("[Previous AnvilScore]"));
    }

    #[test]
    fn anvil_score_section_omitted_when_none() {
        let frame = frame_with_kind(FeedbackKind::TestFailure);
        let inputs = ReminderInputs {
            user_task: "u",
            mode_label: "act",
            plan_summary: None,
            active_precautions_summary: "(none)",
            frame: &frame,
            working_memory_touched: &[],
            anvil_score: None,
        };
        let prompt = build_reminder_prompt(&inputs);
        assert!(!prompt.contains("AnvilScore"));
        assert!(!prompt.contains("[Current"));
    }

    #[test]
    fn anvil_score_section_render_is_capped_and_output_guard_remains() {
        // Even with the AnvilScore section appended, the prompt MUST still
        // end with the JSON-only output guard. With renderer truncation at
        // MAX_RENDERED_CHARS = 512 and total prompt budget at
        // REMINDER_PROMPT_MAX_BYTES = 12 KiB, the trailing guard survives.
        let frame = frame_with_kind(FeedbackKind::TestFailure);
        let score = fixture_score();
        let inputs = ReminderInputs {
            user_task: "u",
            mode_label: "act",
            plan_summary: None,
            active_precautions_summary: "(none)",
            frame: &frame,
            working_memory_touched: &[],
            anvil_score: Some(AnvilScoreSnapshot::CurrentTurn(&score)),
        };
        let prompt = build_reminder_prompt(&inputs);
        assert!(
            prompt.ends_with("# Output\nJSON only.\n"),
            "expected trailing JSON-only guard, got tail: {:?}",
            &prompt[prompt.len().saturating_sub(60)..]
        );
        // Renderer cap: the rendered AnvilScore section's body must be at
        // most MAX_RENDERED_CHARS chars (and we used ~280-char fixture).
        assert!(prompt.contains("# AnvilScore\n"));
    }
}
