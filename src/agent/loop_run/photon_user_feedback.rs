//! Issue #592: User-explicit feedback adapter for photon seeds.
//!
//! Surfaces four slash commands:
//!   * `/photon-thumbs-up`   → `outcome=user_positive`
//!   * `/photon-thumbs-down` → `outcome=user_negative`
//!   * `/photon-correct "<text>"` → `outcome=user_correction` + seed-draft + AntiPattern best-effort
//!   * `/photon-rule    "<text>"` → `outcome=user_rule` + seed-draft (gated by `photon_common_seed_enabled`)
//!
//! Defense in depth (8 layers, matching `feedback_kind_confirm.rs`):
//!   1. Byte cap (`PHOTON_USER_FEEDBACK_INPUT_MAX_BYTES`) — adapter side.
//!   2. ASCII control rejection — `prompt::validate_user_feedback_input`.
//!   3. Bidi / zero-width control rejection — same.
//!   4. `mask_secrets` divergence rejection — same.
//!   5. Secret-word rejection — same.
//!   6. Prompt-injection rejection — same.
//!   7. Destructive-command rejection — same.
//!   8. `serde_json::to_string` JSON escape before embedding in EvaluateRequest
//!      (CB-003): caller-controlled `user_text` is owned by `PhotonSeedDraft`
//!      which `serde_json::to_vec_pretty` will escape on persist.
//!
//! Per-turn cap: `Agent.photon_user_feedback_called_this_turn`, reset in
//! `process_line`. Only consumed by thumbs commands (the correct/rule paths
//! persist + log even when invoked multiple times within a turn so the user
//! can iterate, though approval gates keep multi-call risk low).
//!
//! Logging: 5 suffixes
//!   `agent.photon_feedback.{completed, skipped, failed, disabled, rejected}`
//! Payload keys (7): `session_id`, `turn_index`, `command`, `summary_ids`,
//! `outcome`, `dry_run`, `reason`. All payloads run through
//! `mask_payload_inplace` final defense.

use std::env::VarError;
use std::time::{SystemTime, UNIX_EPOCH};

use super::Agent;
use crate::logging::{log_llm_event, mask_payload_inplace};
use crate::modes::plan_act::ExecutionMode;
use crate::photon::mapper::{FeedbackEvaluateInputs, build_feedback_evaluate_request};
use crate::photon::prompt::{FeedbackInputRejection, validate_user_feedback_input};
use crate::photon::seed_draft::{self, PhotonSeedDraft, derive_draft_id};
use crate::session::feedback::mask_secrets;

// ---------------------------------------------------------------------------
// Module-private constants
// ---------------------------------------------------------------------------

/// Maximum byte length of user-typed feedback after stripping the slash
/// command prefix. Matches `FEEDBACK_KIND_CONFIRM_PROMPT_INPUT_MAX_BYTES`.
const PHOTON_USER_FEEDBACK_INPUT_MAX_BYTES: usize = 8 * 1024;

/// Tail-walk depth when looking for the most recent assistant action that
/// `/photon-correct` should annotate.
const SCAN_LAST_N_MESSAGES: usize = 32;

/// Outcome literals. The photon sidecar treats these as an `OUTCOME_VALUES`
/// allowlist (Deployment Ordering Constraint: photon must accept the four
/// strings before Anvil ships them). Keeping these as `&str` literals avoids
/// a client-side enum that would have to be kept in sync.
const OUTCOME_USER_POSITIVE: &str = "user_positive";
const OUTCOME_USER_NEGATIVE: &str = "user_negative";
const OUTCOME_USER_CORRECTION: &str = "user_correction";
const OUTCOME_USER_RULE: &str = "user_rule";

const COMMAND_THUMBS_UP: &str = "/photon-thumbs-up";
const COMMAND_THUMBS_DOWN: &str = "/photon-thumbs-down";
const COMMAND_CORRECT: &str = "/photon-correct";
const COMMAND_RULE: &str = "/photon-rule";

// ---------------------------------------------------------------------------
// Env gate (closure DI)
// ---------------------------------------------------------------------------

/// `ANVIL_NO_PHOTON_FEEDBACK=<non-empty>` disables every public entry-point.
/// Closure-DI signature mirrors `tester_disabled` / `case_record_disabled`.
pub fn photon_feedback_disabled<F>(get_env: F) -> bool
where
    F: Fn(&str) -> Result<String, VarError>,
{
    matches!(get_env("ANVIL_NO_PHOTON_FEEDBACK"), Ok(v) if !v.is_empty())
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// `/photon-thumbs-up` — record positive feedback for seeds injected last turn.
pub fn handle_thumbs_up(agent: &mut Agent) -> Result<String, String> {
    send_thumbs_signal(agent, OUTCOME_USER_POSITIVE, COMMAND_THUMBS_UP)
}

/// `/photon-thumbs-down` — record negative feedback for seeds injected last turn.
pub fn handle_thumbs_down(agent: &mut Agent) -> Result<String, String> {
    send_thumbs_signal(agent, OUTCOME_USER_NEGATIVE, COMMAND_THUMBS_DOWN)
}

/// `/photon-correct "<text>"` — record an explicit correction.
///
/// Steps:
///   1. env gate / offline check / arg parse
///   2. 8-layer validation
///   3. approval gate (yes_mode or TTY)
///   4. extract failed action summary from session tail
///   5. AntiPattern `extract_or_increment` (best-effort; skip when not eligible)
///   6. persist `PhotonSeedDraft` (skip when Plan mode → dry_run)
///   7. ship to `/v1/evaluate` with `outcome=user_correction`
pub fn handle_correct(agent: &mut Agent, raw_arg: &str) -> Result<String, String> {
    handle_correct_or_rule(agent, raw_arg, FeedbackCommand::Correct)
}

/// `/photon-rule "<text>"` — record a project rule as a photon seed.
///
/// Same pipeline as `handle_correct` but gated by `photon_common_seed_enabled`.
/// When the gate is `false` (default), the call runs as a dry_run (no persist,
/// no evaluate) so users can practice the workflow without polluting the
/// photon corpus.
pub fn handle_rule(agent: &mut Agent, raw_arg: &str) -> Result<String, String> {
    handle_correct_or_rule(agent, raw_arg, FeedbackCommand::Rule)
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeedbackCommand {
    Correct,
    Rule,
}

impl FeedbackCommand {
    fn slash(self) -> &'static str {
        match self {
            Self::Correct => COMMAND_CORRECT,
            Self::Rule => COMMAND_RULE,
        }
    }

    fn outcome(self) -> &'static str {
        match self {
            Self::Correct => OUTCOME_USER_CORRECTION,
            Self::Rule => OUTCOME_USER_RULE,
        }
    }

    fn command_field(self) -> &'static str {
        match self {
            Self::Correct => "correct",
            Self::Rule => "rule",
        }
    }
}

fn send_thumbs_signal(agent: &mut Agent, outcome: &str, command: &str) -> Result<String, String> {
    // Env gate
    if photon_feedback_disabled(|k| std::env::var(k)) {
        emit_event(
            agent,
            "disabled",
            command,
            outcome,
            &[],
            false,
            "env_disabled",
            "",
        );
        return Ok("photon feedback disabled".to_string());
    }
    // Offline / photon-disabled check
    if agent.photon.is_none() || !agent.config.photon_enabled {
        emit_event(
            agent,
            "skipped",
            command,
            outcome,
            &[],
            false,
            "offline",
            "",
        );
        return Ok("photon not configured; feedback ignored".to_string());
    }
    // Per-turn cap
    if agent.photon_user_feedback_called_this_turn {
        emit_event(
            agent,
            "skipped",
            command,
            outcome,
            &[],
            false,
            "per_turn_cap_consumed",
            "",
        );
        return Ok("photon feedback already sent this turn".to_string());
    }
    // Inject presence check
    if agent.last_injected_summary_ids.is_empty() {
        emit_event(
            agent,
            "skipped",
            command,
            outcome,
            &[],
            false,
            "no_injection",
            "",
        );
        return Ok(
            "No photon seeds were injected; nothing to thumb. Try after a turn that uses photon."
                .to_string(),
        );
    }
    // Turn-staleness check (informational; thumbs only fires on the turn
    // immediately following the injection, so the indices should differ by
    // 0 or 1 depending on whether the next turn already ran).
    let inject_turn = agent.last_injected_summary_turn_index;
    let current = agent.current_turn_index;
    if let Some(t) = inject_turn {
        // Allow the same turn (synchronous follow-up) or +1 (typical REPL).
        if current.saturating_sub(t) > 1 {
            emit_event(
                agent,
                "skipped",
                command,
                outcome,
                &[],
                false,
                "stale_turn",
                "",
            );
            return Ok("inject signal is too old; thumbs ignored".to_string());
        }
    }

    let ids = agent.last_injected_summary_ids.clone();
    let feedback_event_id =
        generate_feedback_event_id(agent.session_store.session_id(), current, command);
    let cp_id = agent.last_context_pack_id.clone();
    let inputs = FeedbackEvaluateInputs {
        session_id: agent.session_store.session_id(),
        turn_index: current,
        command,
        summary_ids: &ids,
        outcome,
        draft_id: None,
        source_context_pack_request_id: cp_id.as_deref(),
        feedback_event_id: &feedback_event_id,
    };
    let req = build_feedback_evaluate_request(&inputs);
    let client = agent
        .photon
        .as_ref()
        .expect("photon presence checked above");
    let result = client.evaluate(&req);

    // Consume cap + clear ids regardless of HTTP outcome (one-shot per turn).
    agent.photon_user_feedback_called_this_turn = true;
    agent.last_injected_summary_ids.clear();

    if result.is_some() {
        emit_event(
            agent,
            "completed",
            command,
            outcome,
            &ids,
            false,
            "ok",
            &feedback_event_id,
        );
        Ok(format!("photon feedback recorded ({outcome})"))
    } else {
        emit_event(
            agent,
            "failed",
            command,
            outcome,
            &ids,
            false,
            "evaluate_failed",
            &feedback_event_id,
        );
        Ok("photon feedback dispatch failed (fail-open; no retry)".to_string())
    }
}

fn handle_correct_or_rule(
    agent: &mut Agent,
    raw_arg: &str,
    kind: FeedbackCommand,
) -> Result<String, String> {
    let command = kind.slash();
    let outcome = kind.outcome();

    // Env gate
    if photon_feedback_disabled(|k| std::env::var(k)) {
        emit_event(
            agent,
            "disabled",
            command,
            outcome,
            &[],
            false,
            "env_disabled",
            "",
        );
        return Ok("photon feedback disabled".to_string());
    }
    // Offline check
    if agent.photon.is_none() || !agent.config.photon_enabled {
        emit_event(
            agent,
            "skipped",
            command,
            outcome,
            &[],
            false,
            "offline",
            "",
        );
        return Ok("photon not configured; feedback ignored".to_string());
    }

    // Argument parse
    let user_text = match parse_double_quoted_argument(raw_arg) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            emit_event(
                agent,
                "rejected",
                command,
                outcome,
                &[],
                false,
                "empty_argument",
                "",
            );
            return Ok(format!(
                "usage: {command} \"<text>\" — the text must be quoted and non-empty"
            ));
        }
    };

    // Layer 1: byte cap.
    if user_text.len() > PHOTON_USER_FEEDBACK_INPUT_MAX_BYTES {
        emit_event(
            agent,
            "rejected",
            command,
            outcome,
            &[],
            false,
            "byte_cap",
            "",
        );
        return Ok(format!(
            "input exceeds {PHOTON_USER_FEEDBACK_INPUT_MAX_BYTES} bytes"
        ));
    }

    // Layers 2-7: SSOT validator.
    if let Err(rej) = validate_user_feedback_input(&user_text) {
        emit_event(
            agent,
            "rejected",
            command,
            outcome,
            &[],
            false,
            rejection_reason(rej),
            "",
        );
        return Ok(format!("input rejected: {}", rejection_reason(rej)));
    }

    // Layer 8: JSON-escape via serde happens automatically when the
    // PhotonSeedDraft is `serde_json::to_vec_pretty`'d in `seed_draft::persist`
    // and when `EvaluateRequest` is sent (the wrapper is a `serde_json::Value`
    // that goes through `reqwest::json`). No raw concatenation occurs here.

    // CB-001 (Issue #592): approval gate. `yes_mode` (`--yes` / `ANVIL_YES`)
    // bypasses the prompt. Otherwise the design (S4-003) requires an explicit
    // interactive y/yes from the user; default deny on any error / EOF / non-y
    // input, and reject outright when no TTY is attached.
    if !agent.config.yes_mode && !prompt_approval(command, &kind, agent.footer.clone()) {
        emit_event(
            agent,
            "rejected",
            command,
            outcome,
            &[],
            false,
            "approval_denied",
            "",
        );
        return Ok("approval required: rerun with --yes or answer y/yes at the prompt".to_string());
    }

    // Plan mode → dry-run (skip persist + evaluate). The rule gate (S5)
    // also forces dry-run for /photon-rule when photon_common_seed_enabled
    // is false.
    let plan_dry_run = agent.session.mode_state.mode == ExecutionMode::Plan;
    let rule_gate_dry_run =
        matches!(kind, FeedbackCommand::Rule) && !agent.config.photon_common_seed_enabled;
    let dry_run = plan_dry_run || rule_gate_dry_run;

    // Build seed draft
    let now_ms = current_unix_ms();
    let cp_id = agent.last_context_pack_id.clone();
    let originating = agent.last_injected_summary_ids.first().cloned();
    let payload_for_id = format!("{}|{}|{}", command, &user_text, now_ms);
    let draft_id = derive_draft_id(&payload_for_id, now_ms);
    let feedback_event_id = generate_feedback_event_id(
        agent.session_store.session_id(),
        agent.current_turn_index,
        command,
    );

    let repo_fingerprint = match kind {
        FeedbackCommand::Rule => Some(crate::session::case_record::capture_repo_fingerprint(
            &agent.session.workspace_key,
            &agent.work_root,
            &[],
        )),
        FeedbackCommand::Correct => None,
    };

    let draft = PhotonSeedDraft {
        draft_id: draft_id.clone(),
        created_at: now_ms as u64,
        command: kind.command_field().to_string(),
        repo_fingerprint,
        source_context_pack_request_id: cp_id.clone(),
        originating_summary_id: originating,
        user_text: user_text.clone(),
        feedback_event_id: feedback_event_id.clone(),
    };

    // For /photon-correct: best-effort AntiPattern record.
    if matches!(kind, FeedbackCommand::Correct) && !dry_run {
        try_record_anti_pattern_best_effort(agent, &user_text, command, outcome);
    }

    if dry_run {
        let reason = if plan_dry_run {
            "plan_mode_dry_run"
        } else {
            "common_seed_disabled_dry_run"
        };
        emit_event(
            agent,
            "skipped",
            command,
            outcome,
            &[],
            true,
            reason,
            &feedback_event_id,
        );
        return Ok(format!("{command} dry-run ({reason})"));
    }

    // Persist
    let state_root = agent.session_store.state_root().to_path_buf();
    if let Err(e) = seed_draft::persist(&state_root, &draft) {
        let reason = match e {
            seed_draft::SeedDraftPersistError::TooBig { .. } => "persist_too_big",
            seed_draft::SeedDraftPersistError::Io(_) => "persist_io",
            seed_draft::SeedDraftPersistError::InvalidId(_) => "persist_invalid_id",
        };
        emit_event(
            agent,
            "failed",
            command,
            outcome,
            &[],
            false,
            reason,
            &feedback_event_id,
        );
        return Ok(format!("seed draft persist failed ({reason})"));
    }

    // Evaluate
    let inputs = FeedbackEvaluateInputs {
        session_id: agent.session_store.session_id(),
        turn_index: agent.current_turn_index,
        command,
        summary_ids: &[],
        outcome,
        draft_id: Some(&draft_id),
        source_context_pack_request_id: cp_id.as_deref(),
        feedback_event_id: &feedback_event_id,
    };
    let req = build_feedback_evaluate_request(&inputs);
    let client = agent
        .photon
        .as_ref()
        .expect("photon presence checked above");
    let result = client.evaluate(&req);

    if result.is_some() {
        emit_event(
            agent,
            "completed",
            command,
            outcome,
            &[],
            false,
            "ok",
            &feedback_event_id,
        );
        Ok(format!(
            "photon {} recorded ({outcome})",
            kind.command_field()
        ))
    } else {
        emit_event(
            agent,
            "failed",
            command,
            outcome,
            &[],
            false,
            "evaluate_failed",
            &feedback_event_id,
        );
        Ok("photon feedback dispatch failed (fail-open; no retry)".to_string())
    }
}

/// CB-001 (Issue #592): interactive approval gate for `/photon-correct` and
/// `/photon-rule`. Returns `true` ONLY when the user types an exact `y` or
/// `yes` (case-insensitive, after trim) on an attached TTY. Any other input —
/// EOF, empty line, read error, no TTY on either side — is fail-closed.
///
/// The footer worker is paused via `freeze_for_prompt` so the prompt line is
/// not overwritten by the live status bar. On non-unix or when no footer is
/// attached the freeze is a no-op.
fn prompt_approval(command: &str, kind: &FeedbackCommand, footer: super::FooterHandle) -> bool {
    use std::io::{BufRead, IsTerminal, Write};

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    if !stdin.is_terminal() || !stdout.is_terminal() {
        return false;
    }

    // Pause the footer so the prompt is readable.
    let _freeze = footer.freeze_for_prompt();

    // Render a context-appropriate verb so the user knows what they are
    // approving (correction vs project rule).
    let verb = match kind {
        FeedbackCommand::Correct => "correction",
        FeedbackCommand::Rule => "rule",
    };
    {
        let mut out = stdout.lock();
        if write!(out, "Apply this {verb} ({command})? [y/N]: ").is_err() {
            return false;
        }
        if out.flush().is_err() {
            return false;
        }
    }

    let mut line = String::new();
    let read_result = stdin.lock().read_line(&mut line);
    match read_result {
        Ok(0) => false, // EOF
        Ok(_) => {
            let trimmed = line.trim().to_ascii_lowercase();
            trimmed == "y" || trimmed == "yes"
        }
        Err(_) => false,
    }
}

/// Strip the slash command prefix (if present) and outer double-quotes.
/// Returns the trimmed inner string, or `None` when the syntax is malformed.
fn parse_double_quoted_argument(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // The dispatch already strips the command keyword and one space; some
    // callers may still pass the full input. Walk past a leading `/photon-...`
    // token to be defensive.
    let after_cmd = if let Some(rest) = trimmed.strip_prefix(COMMAND_CORRECT) {
        rest.trim_start()
    } else if let Some(rest) = trimmed.strip_prefix(COMMAND_RULE) {
        rest.trim_start()
    } else {
        trimmed
    };
    // Accept either `"..."` or unquoted single token (latter is treated as
    // valid for clipboard friendliness but the typical UX is to quote).
    let stripped = after_cmd
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'));
    match stripped {
        Some(inner) => Some(inner),
        None => {
            // Mismatched / partial quotes are an error; otherwise treat as a
            // bare token for clipboard friendliness.
            let mismatched = after_cmd.starts_with('"') || after_cmd.ends_with('"');
            if mismatched || after_cmd.is_empty() {
                None
            } else {
                Some(after_cmd)
            }
        }
    }
}

/// Best-effort: try to drive `anti_pattern::extract_or_increment` with the
/// most recent assistant action. Failures are logged at debug level only; the
/// caller never propagates the error to the user.
///
/// CB-007 (Issue #592): consult `session.last_feedback.kind` per S7-002. If
/// no eligible feedback frame is on record (`None` or a kind for which
/// `anti_pattern::is_repeat_eligible_kind` is false) we skip the AntiPattern
/// call entirely and emit a `agent.photon_feedback.skipped` event with
/// `reason=no_eligible_feedback_kind`. Seed-draft persistence and the
/// evaluate signal still proceed as configured by the caller.
fn try_record_anti_pattern_best_effort(
    agent: &mut Agent,
    user_text: &str,
    command: &str,
    outcome: &str,
) {
    let active_task = agent.session.working_memory.active_task.clone();
    if active_task.is_none() {
        return;
    }
    // CB-007: only proceed when we have a recent eligible failure frame.
    let feedback_kind = match agent.session.last_feedback.as_ref() {
        Some(frame) if crate::session::anti_pattern::is_repeat_eligible_kind(&frame.kind) => {
            frame.kind.clone()
        }
        _ => {
            emit_event(
                agent,
                "skipped",
                command,
                outcome,
                &[],
                false,
                "no_eligible_feedback_kind",
                "",
            );
            return;
        }
    };
    let failed_summary = extract_failed_action_summary(&agent.session.messages)
        .unwrap_or_else(|| user_text.to_string());
    let masked = mask_secrets(&failed_summary);
    let touched: Vec<String> = agent.session.working_memory.touched_files.clone();
    let inputs = crate::session::anti_pattern::AntiPatternRecordInputs {
        workspace_key: &agent.session.workspace_key,
        work_root: &agent.work_root,
        active_task: active_task.as_deref(),
        language_stack: &[],
        touched_files: &touched,
        feedback_kind,
        failed_action_summary: &masked,
    };
    let _ = crate::session::anti_pattern::extract_or_increment(
        agent.session_store.state_root(),
        &inputs,
    );
}

/// Walk back from the tail looking for the most recent assistant message
/// (with tool calls or content) to summarise as the "what just happened"
/// hook for `/photon-correct`.
fn extract_failed_action_summary(
    messages: &[crate::session::store::ConversationMessage],
) -> Option<String> {
    let take = messages.len().min(SCAN_LAST_N_MESSAGES);
    let start = messages.len().saturating_sub(take);
    let tail = &messages[start..];
    for msg in tail.iter().rev() {
        if msg.role == "assistant" {
            if !msg.tool_calls.is_empty() {
                let names: Vec<String> = msg.tool_calls.iter().map(|t| t.name.clone()).collect();
                return Some(format!("tool_calls={}", names.join(",")));
            }
            if !msg.content.trim().is_empty() {
                let trimmed: String = msg.content.chars().take(240).collect();
                return Some(trimmed);
            }
        }
    }
    None
}

fn current_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Process-wide monotonic counter used to defeat within-millisecond hash
/// collisions on `generate_feedback_event_id`. CB-006 (Issue #592).
static FEEDBACK_EVENT_ID_COUNTER: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Deterministic-ish unique feedback_event id derived from `(session_id, turn,
/// command, now_ms, now_nanos, process_counter)`. CB-006 (Issue #592): the
/// nanosecond timestamp and atomic counter eliminate the within-millisecond
/// collision window that the original `_ms` only hash had. Avoids adding a
/// new uuid dependency (the existing v7 uuid is also acceptable but tying it
/// to the call site keeps log joins simple).
fn generate_feedback_event_id(session_id: &str, turn_index: usize, command: &str) -> String {
    use sha2::{Digest, Sha256};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let now_ms = now.as_millis() as i64;
    let now_nanos = now.as_nanos();
    let counter = FEEDBACK_EVENT_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut hasher = Sha256::new();
    hasher.update(session_id.as_bytes());
    hasher.update(b"\x00");
    hasher.update(turn_index.to_le_bytes());
    hasher.update(b"\x00");
    hasher.update(command.as_bytes());
    hasher.update(b"\x00");
    hasher.update(now_ms.to_le_bytes());
    hasher.update(b"\x00");
    hasher.update(now_nanos.to_le_bytes());
    hasher.update(b"\x00");
    hasher.update(counter.to_le_bytes());
    let digest = hasher.finalize();
    let hex = format!("{digest:x}");
    format!("fb_event_{}", &hex[..32])
}

fn rejection_reason(r: FeedbackInputRejection) -> &'static str {
    r.as_reason_str()
}

/// CB-006 (Issue #592): emit a `agent.photon_feedback.<suffix>` event with
/// 8 keys including `feedback_event_id` (optional — empty string when the
/// event fires before an id is minted, e.g. validation rejection or env
/// disable). Inclusion lets operators correlate completed/failed events with
/// the evaluate-request payload that ships the same id.
#[allow(clippy::too_many_arguments)]
fn emit_event(
    agent: &Agent,
    suffix: &str,
    command: &str,
    outcome: &str,
    summary_ids: &[String],
    dry_run: bool,
    reason: &str,
    feedback_event_id: &str,
) {
    let event = match suffix {
        "completed" => "agent.photon_feedback.completed",
        "skipped" => "agent.photon_feedback.skipped",
        "failed" => "agent.photon_feedback.failed",
        "disabled" => "agent.photon_feedback.disabled",
        "rejected" => "agent.photon_feedback.rejected",
        _ => "agent.photon_feedback.skipped",
    };
    let masked_ids: Vec<String> = summary_ids.iter().map(|s| mask_secrets(s)).collect();
    let mut payload = serde_json::json!({
        "session_id": agent.session_store.session_id(),
        "turn_index": agent.current_turn_index,
        "command": command,
        "summary_ids": masked_ids,
        "outcome": outcome,
        "dry_run": dry_run,
        "reason": reason,
        "feedback_event_id": feedback_event_id,
    });
    mask_payload_inplace(&mut payload);
    log_llm_event(event, payload);
}

// ---------------------------------------------------------------------------
// Module unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photon_feedback_disabled_respects_env() {
        assert!(photon_feedback_disabled(|_| Ok::<_, VarError>(
            "1".to_string()
        )));
        assert!(!photon_feedback_disabled(|_| Ok::<_, VarError>(
            "".to_string()
        )));
        assert!(!photon_feedback_disabled(|_| Err(VarError::NotPresent)));
    }

    #[test]
    fn parse_double_quoted_argument_basic() {
        assert_eq!(
            parse_double_quoted_argument("\"hello world\""),
            Some("hello world")
        );
        assert_eq!(parse_double_quoted_argument("  \"q\"  "), Some("q"));
    }

    #[test]
    fn parse_double_quoted_argument_unquoted_passthrough() {
        // Allowing unquoted is intentional for clipboard friendliness.
        assert_eq!(parse_double_quoted_argument("no quotes"), Some("no quotes"));
    }

    #[test]
    fn parse_double_quoted_argument_mismatched_returns_none() {
        assert_eq!(parse_double_quoted_argument("\"unbalanced"), None);
        assert_eq!(parse_double_quoted_argument("unbalanced\""), None);
    }

    #[test]
    fn parse_double_quoted_argument_empty_returns_none() {
        assert_eq!(parse_double_quoted_argument(""), None);
        assert_eq!(parse_double_quoted_argument("   "), None);
    }

    #[test]
    fn parse_double_quoted_argument_strips_command_prefix() {
        let s = format!("{COMMAND_CORRECT} \"text\"");
        assert_eq!(parse_double_quoted_argument(&s), Some("text"));
        let s = format!("{COMMAND_RULE} \"abc def\"");
        assert_eq!(parse_double_quoted_argument(&s), Some("abc def"));
    }

    #[test]
    fn generate_feedback_event_id_is_unique_per_call() {
        let a = generate_feedback_event_id("s", 1, COMMAND_THUMBS_UP);
        // Tiny sleep so the ms-timestamp differs (deterministic call w/o sleep
        // could collide). We test format here, not uniqueness.
        assert!(a.starts_with("fb_event_"));
        assert!(a.len() > "fb_event_".len() + 16);
    }

    #[test]
    fn rejection_reason_strings_match_enum() {
        assert_eq!(
            rejection_reason(FeedbackInputRejection::AsciiControl),
            "ascii_control"
        );
        assert_eq!(
            rejection_reason(FeedbackInputRejection::PromptInjection),
            "prompt_injection"
        );
    }

    #[test]
    fn outcome_literals_are_distinct() {
        let outcomes = [
            OUTCOME_USER_POSITIVE,
            OUTCOME_USER_NEGATIVE,
            OUTCOME_USER_CORRECTION,
            OUTCOME_USER_RULE,
        ];
        let mut seen = std::collections::HashSet::new();
        for o in outcomes {
            assert!(seen.insert(o), "duplicate outcome literal: {o}");
        }
    }
}
