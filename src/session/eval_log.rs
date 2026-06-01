//! Structured evaluation log (Issue #471).
//!
//! Writes one JSON-Lines record per completed turn to `logs/eval.jsonl`.
//! Complements the raw LLM I/O stream in `llm-io.jsonl` with a turn-level
//! aggregate snapshot suitable for dataset export and offline analysis.
//!
//! Security processing order (DR4-001):
//!   `serde_json::to_value` → `mask_payload_inplace` → `serde_json::to_string`
//!   → optional `scrub_absolute_paths` → size check → append.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use crate::logging::mask_payload_inplace;
use crate::session::anvil_score::AnvilScore;
use crate::session::feedback::mask_secrets;
use crate::session::precaution::{Precaution, PrecautionStatus};

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

pub const MAX_EVAL_LOG_RECORD_BYTES: usize = 64 * 1024;
pub const MAX_EVAL_TASK_BYTES: usize = 4096;
pub const MAX_EVAL_TOOL_ARG_BYTES: usize = 1024;
pub const MAX_EVAL_FEEDBACK_EXCERPT_BYTES: usize = 8192;
pub const MAX_EVAL_PRECAUTIONS: usize = 8;
pub const MAX_EVAL_PRECAUTIONS_CHARS: usize = 1024;
pub const MAX_EVAL_VERIFY_CMD_BYTES: usize = 4096;
pub const MAX_PHOTON_EVAL_FIELD_BYTES: usize = 256;
pub const MAX_PHOTON_EVAL_WARNINGS: usize = 8;
pub const MAX_PHOTON_EVAL_WARNING_BYTES: usize = 512;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Turn-level aggregate snapshot written to `logs/eval.jsonl`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EvalRecord {
    pub schema_version: u8,
    pub session_id: String,
    pub ts_ms: u64,
    pub task: String,
    pub model: String,
    pub mode: String,
    pub tool_protocol: String,
    pub tool_calls: Vec<ToolCallSummary>,
    pub feedback_frame: Option<FeedbackFrameSummary>,
    pub active_precautions: Vec<EvalPrecautionSnapshot>,
    pub anvil_score: Option<AnvilScoreSummary>,
    pub changed_file_classes: ChangedFileClasses,
    pub verify_commands: Vec<String>,
    pub case_retrieval_result: Option<CaseRetrievalSummary>,
    pub photon_eval: Option<PhotonEvalSummary>,
    /// Photon canary value (0-1000) recorded at turn time.
    /// 0 = disabled, 1000 = full traffic.
    #[serde(default)]
    pub photon_canary: u16,
    /// Issue #604 (Task 4.1 / DR2-007 / DR1-020): post-loop auto-promote hook
    /// outcome summary attached to this turn. `None` when the hook was not
    /// invoked or did not produce an outcome (e.g. Plan mode / Issue #604
    /// pre-rollout). Omitted from JSON via `skip_serializing_if` so legacy
    /// consumers parsing pre-#604 records remain compatible (Issue #471
    /// schema invariant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_promote: Option<AutoPromoteOutcomeSummary>,
    /// Issue #848: obligation-level diagnosis for `final_outcome`.
    ///
    /// Additive eval-log field so downstream evaluators can classify terminal
    /// failures without reconstructing model/control/verifier state from free
    /// text or sibling event streams. Job-report and safe-stop schemas are
    /// intentionally unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_diagnostics: Option<TerminalDiagnosticsSummary>,
    pub final_outcome: String,
}

/// Summary of a single LLM-requested tool call (no result).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolCallSummary {
    pub name: String,
    pub args_summary: String,
}

/// Minimal public face of a `FeedbackFrame`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FeedbackFrameSummary {
    pub kind: String,
    pub excerpt: String,
}

/// Type-safe summary of `AnvilScore`'s 12 public fields (DR1-002).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AnvilScoreSummary {
    pub build_passed: Option<bool>,
    pub tests_passed: Option<bool>,
    pub compile_errors_delta: Option<i32>,
    pub test_failures_delta: Option<i32>,
    pub compile_error_count: Option<usize>,
    pub test_failure_count: Option<usize>,
    pub implementation_files_changed: Option<usize>,
    pub test_files_changed: Option<usize>,
    pub setup_files_changed: Option<usize>,
    pub unsafe_actions_blocked: usize,
    pub consecutive_no_progress_turns: usize,
    pub user_visible_artifact: bool,
}

impl From<&AnvilScore> for AnvilScoreSummary {
    fn from(s: &AnvilScore) -> Self {
        Self {
            build_passed: s.build_passed,
            tests_passed: s.tests_passed,
            compile_errors_delta: s.compile_errors_delta,
            test_failures_delta: s.test_failures_delta,
            compile_error_count: s.compile_error_count,
            test_failure_count: s.test_failure_count,
            implementation_files_changed: s.implementation_files_changed,
            test_files_changed: s.test_files_changed,
            setup_files_changed: s.setup_files_changed,
            unsafe_actions_blocked: s.unsafe_actions_blocked,
            consecutive_no_progress_turns: s.consecutive_no_progress_turns,
            user_visible_artifact: s.user_visible_artifact,
        }
    }
}

/// Eval-specific 4-field precaution snapshot (DR1-003 / DR2-001).
/// No `kind` field — `Precaution` has no such field.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EvalPrecautionSnapshot {
    pub id: String,
    pub source: String,
    pub severity: String,
    pub status: String,
}

impl From<&Precaution> for EvalPrecautionSnapshot {
    fn from(p: &Precaution) -> Self {
        let status = match p.status {
            PrecautionStatus::Active => "active",
            PrecautionStatus::Resolved => "resolved",
            PrecautionStatus::Retired => "retired",
            PrecautionStatus::Unknown => "unknown",
        };
        Self {
            id: p.id.clone(),
            source: p.source.as_label().to_string(),
            severity: p.severity.as_label().to_string(),
            status: status.to_string(),
        }
    }
}

/// Classification of changed files by type.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChangedFileClasses {
    pub test: usize,
    #[serde(rename = "impl")]
    pub impl_files: usize,
    pub setup: usize,
}

/// Summary of a case retrieval result (DR1-004: reuses `CaseScoreBreakdown`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaseRetrievalSummary {
    pub selected: usize,
    pub scores: Vec<crate::session::case_retrieval::CaseScoreBreakdown>,
}

/// Photon evaluate result summary (Issue #558).
/// Defined in the session layer to keep the layer dependency
/// session←photon, not photon←session (DR3-002).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PhotonEvalSummary {
    pub photon_request_id: Option<String>,
    pub context_pack_id: Option<String>,
    pub admission_decision: Option<String>,
    pub warnings: Vec<String>,
    pub prompt_adopted: Option<bool>,
    pub task_outcome: Option<String>,
    pub retry_summary: Option<String>,
    /// Issue #591 (VR-08): number of `summary_ids_adopted` actually sent to
    /// photon `/v1/evaluate` for this turn (post `sanitize_summary_id` re-run
    /// and post `MAX_PHOTON_EVAL_ADOPTED_IDS` cap). `None` means either
    /// fail-open (no evaluate response, photon disabled) or shadow mode (the
    /// agent layer may populate `Some(0)` for shadow turns; constructor
    /// defaults to `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_ids_adopted_count: Option<usize>,
    /// Issue #601 (S5-003 / 設計判断 #1 (B)): agent-side `outcome` value
    /// (`"success"` / `"failure"` / `"safety_violation"`) actually emitted by
    /// the agent to photon `/v1/evaluate`. `None` means either fail-open (no
    /// evaluate response, photon disabled) or `derive_photon_feedback_outcome`
    /// returned `outcome: None` for this turn. Used by the fine-tuning
    /// dataset / A-0 evaluation to identify no-progress turns at scale.
    /// `#[serde(default)]` maintains backward compatibility with eval.jsonl
    /// files written before Issue #601 (#471 EvalRecord schema invariant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_emitted: Option<String>,
    /// Issue #601 (S5-003 / 設計判断 #1 (B)): agent-side `outcome_detail`
    /// value (`"no_progress_despite_inject"`) actually emitted by the agent.
    /// Combined with `outcome_emitted`, downstream tooling can mechanically
    /// identify Case F no-progress turns. `#[serde(default,
    /// skip_serializing_if = "Option::is_none")]` maintains backward compat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_detail_emitted: Option<String>,
}

/// Issue #604 (Task 4.1 / DR2-007): 3-field flat summary attached to
/// `EvalRecord.auto_promote`. SSOT for the post-loop auto-promote hook's
/// per-turn outcome carrier.
///
/// **3 fields by design** (DR2-007): `request_id` / `scrub_status` /
/// `scrubbed_fields` / `fail_reason` / `skip_sub_reason` are event-payload
/// only and intentionally not duplicated into `EvalRecord` (Issue §AP-12 /
/// EvalRecord schema bloat抑止). The agent layer
/// (`src/agent/loop_run/auto_promote.rs`) re-exports this type via
/// `pub use` to maintain the agent → session layer direction (DR3-002).
///
/// `is_secret_like_key=false` is satisfied: `decision` / `skip_reason` /
/// `summary_id` are all non-secret keys (DR2-025).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AutoPromoteOutcomeSummary {
    /// `"promoted" | "skipped" | "scrubbed" | "failed" | "rejected_by_photon"`
    /// (5-value enum). SSOT lives in `auto_promote::AutoPromoteOutcomeKind::decision_str()`
    /// (Task 4.2 forward-decl).
    pub decision: String,
    /// `AutoPromoteSkipReason::event_str()` short tag (skip-family only is Some).
    pub skip_reason: Option<String>,
    /// Sanitized photon `summary_id` for the promoted / scrubbed /
    /// rejected_by_photon cases. `None` for non-HTTP skip paths.
    pub summary_id: Option<String>,
}

/// Issue #848: structured terminal-outcome classification for eval logs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TerminalDiagnosticsSummary {
    pub outcome: String,
    pub classification: String,
    pub obligations: Vec<TerminalObligationDiagnostic>,
}

/// One obligation status within a terminal outcome.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TerminalObligationDiagnostic {
    pub id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_domain: Option<String>,
    pub detail: String,
}

// ---------------------------------------------------------------------------
// OnceLock state
// ---------------------------------------------------------------------------

static EVAL_LOG_LOGGER: OnceLock<Mutex<File>> = OnceLock::new();

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize `EVAL_LOG_LOGGER`. Opens (or creates) `log_path` for append.
/// Sets Unix permissions to `0o600`. Returns `Err` only for caller warning;
/// the caller (`lib.rs`) must NOT propagate the error (DR3-002).
pub fn init_eval_log(log_path: &Path) -> Result<(), String> {
    match OpenOptions::new().create(true).append(true).open(log_path) {
        Ok(file) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(log_path, std::fs::Permissions::from_mode(0o600));
            }
            let _ = EVAL_LOG_LOGGER.set(Mutex::new(file));
            Ok(())
        }
        Err(err) => Err(format!(
            "failed to open eval log {}: {err}",
            log_path.display()
        )),
    }
}

/// Serialize `record`, apply security processing, and append to `eval.jsonl`.
/// Failures are warn-only (DR3-002).
pub fn write_eval_record(record: &EvalRecord) {
    let Some(logger) = EVAL_LOG_LOGGER.get() else {
        return;
    };
    write_eval_record_to(record, logger);
}

/// Write `record` directly to `file` (bypasses global `EVAL_LOG_LOGGER`).
/// Intended for integration tests that need isolation from the process-global logger.
pub fn write_eval_record_to(record: &EvalRecord, file: &Mutex<File>) {
    // DR4-001: to_value → mask → to_string → optional path scrub → size check → append
    let mut value = match serde_json::to_value(record) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("warning: eval_log serialize error: {err}");
            return;
        }
    };
    mask_payload_inplace(&mut value);
    let json = match serde_json::to_string(&value) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("warning: eval_log to_string error: {err}");
            return;
        }
    };
    let json = if std::env::var_os("ANVIL_EVAL_SCRUB_PATHS").is_some() {
        scrub_absolute_paths(&json)
    } else {
        json
    };
    if json.len() > MAX_EVAL_LOG_RECORD_BYTES {
        eprintln!(
            "warning: eval_log record too large ({} bytes > {}), dropping",
            json.len(),
            MAX_EVAL_LOG_RECORD_BYTES
        );
        return;
    }
    if let Ok(mut guard) = file.lock() {
        let _ = writeln!(guard, "{json}");
    }
}

/// Pure assembly function for `EvalRecord` (DR1-001).
/// `ts_ms` is passed by the caller so this function is side-effect-free and
/// unit-testable without mocking `SystemTime`.
#[allow(clippy::too_many_arguments)]
pub fn build_eval_record(
    session_id: &str,
    ts_ms: u64,
    task: &str,
    model: &str,
    mode: &str,
    tool_protocol: &str,
    tool_calls: &[ToolCallSummary],
    feedback_frame: Option<FeedbackFrameSummary>,
    active_precautions: &[EvalPrecautionSnapshot],
    anvil_score: Option<AnvilScoreSummary>,
    changed_file_classes: ChangedFileClasses,
    verify_commands: &[String],
    case_retrieval_result: Option<CaseRetrievalSummary>,
    photon_eval: Option<PhotonEvalSummary>,
    auto_promote: Option<AutoPromoteOutcomeSummary>,
    final_outcome: &str,
) -> EvalRecord {
    // DR4-002: mask_secrets → truncate for free-text fields
    let task = truncate_bytes(&mask_secrets(task), MAX_EVAL_TASK_BYTES);
    let verify_commands: Vec<String> = verify_commands
        .iter()
        .map(|c| truncate_bytes(&mask_secrets(c), MAX_EVAL_VERIFY_CMD_BYTES))
        .collect();
    let terminal_diagnostics = Some(build_terminal_diagnostics(
        final_outcome,
        &changed_file_classes,
        verify_commands.len(),
    ));

    // Precautions: cap count
    let active_precautions: Vec<EvalPrecautionSnapshot> = active_precautions
        .iter()
        .take(MAX_EVAL_PRECAUTIONS)
        .cloned()
        .collect();

    EvalRecord {
        schema_version: 1,
        session_id: session_id.to_string(),
        ts_ms,
        task,
        model: model.to_string(),
        mode: mode.to_string(),
        tool_protocol: tool_protocol.to_string(),
        tool_calls: tool_calls.to_vec(),
        feedback_frame,
        active_precautions,
        anvil_score,
        changed_file_classes,
        verify_commands,
        case_retrieval_result,
        photon_eval,
        photon_canary: 0,
        auto_promote,
        terminal_diagnostics,
        final_outcome: final_outcome.to_string(),
    }
}

pub fn build_terminal_diagnostics(
    final_outcome: &str,
    changed_file_classes: &ChangedFileClasses,
    verify_command_count: usize,
) -> TerminalDiagnosticsSummary {
    let classification = classify_terminal_outcome(final_outcome);
    let changed_count = changed_file_classes
        .test
        .saturating_add(changed_file_classes.impl_files)
        .saturating_add(changed_file_classes.setup);
    let repo_edit_status = if changed_count > 0 {
        ("satisfied", "repository edits were recorded")
    } else {
        ("not_observed", "no repository edits were recorded")
    };
    let verifier_status = if verify_command_count > 0 {
        ("satisfied", "verifier command evidence was recorded")
    } else {
        ("not_observed", "no verifier command evidence was recorded")
    };

    let mut obligations = vec![
        obligation(
            "model_output_format",
            "satisfied",
            None,
            "model output was parseable enough to reach terminal handling",
        ),
        obligation("repo_edit", repo_edit_status.0, None, repo_edit_status.1),
        obligation(
            "verification_environment",
            verifier_status.0,
            None,
            verifier_status.1,
        ),
        obligation(
            "verification_evidence",
            verifier_status.0,
            None,
            verifier_status.1,
        ),
        obligation(
            "repair_convergence",
            "not_applicable",
            None,
            "repair loop was not the terminal authority",
        ),
    ];

    match final_outcome {
        "done" => {
            if changed_count == 0 {
                set_obligation(
                    &mut obligations,
                    "repo_edit",
                    "not_applicable",
                    None,
                    "no repository edit obligation was observed for this successful turn",
                );
            }
            if verify_command_count == 0 {
                set_obligation(
                    &mut obligations,
                    "verification_environment",
                    "not_applicable",
                    None,
                    "no verifier obligation was observed for this successful turn",
                );
                set_obligation(
                    &mut obligations,
                    "verification_evidence",
                    "not_applicable",
                    None,
                    "no verifier evidence obligation was observed for this successful turn",
                );
            }
        }
        "missing_repo_edits" => {
            set_obligation(
                &mut obligations,
                "repo_edit",
                "unsatisfied",
                Some("model_output_failure"),
                "terminal outcome reports that required repository edits were not produced",
            );
            set_obligation(
                &mut obligations,
                "verification_environment",
                "not_applicable",
                None,
                "verification environment was not the terminal blocker",
            );
            set_obligation(
                &mut obligations,
                "verification_evidence",
                "not_applicable",
                None,
                "verification evidence was not reached because repository edits were missing",
            );
        }
        "tool_call_format_error" => {
            set_obligation(
                &mut obligations,
                "model_output_format",
                "unsatisfied",
                Some("model_output_failure"),
                "assistant emitted malformed or truncated tool calls repeatedly",
            );
            set_obligation(
                &mut obligations,
                "verification_environment",
                "not_applicable",
                None,
                "verification environment was not the terminal blocker",
            );
            set_obligation(
                &mut obligations,
                "verification_evidence",
                "not_applicable",
                None,
                "verification evidence was not reached because tool-call parsing failed",
            );
        }
        "safe_stop_verifier_missing" => {
            set_obligation(
                &mut obligations,
                "verification_environment",
                "unsatisfied",
                Some("verification_environment_failure"),
                "requested verification could not run because no authoritative verifier was available",
            );
            set_obligation(
                &mut obligations,
                "verification_evidence",
                "unsatisfied",
                Some("verification_environment_failure"),
                "verification evidence is absent because verifier setup is missing",
            );
        }
        "repair_exhausted" => {
            set_obligation(
                &mut obligations,
                "repair_convergence",
                "unsatisfied",
                Some("control_loop_failure"),
                "verifier repair reached its controlled exhaustion terminal",
            );
        }
        _ => {}
    }

    TerminalDiagnosticsSummary {
        outcome: final_outcome.to_string(),
        classification: classification.to_string(),
        obligations,
    }
}

fn classify_terminal_outcome(final_outcome: &str) -> &'static str {
    match final_outcome {
        "done" => "success",
        "missing_repo_edits" | "tool_call_format_error" => "model_output_failure",
        "safe_stop_verifier_missing" => "verification_environment_failure",
        "repair_exhausted" => "control_loop_failure",
        "missing_verification" | "verifier_failed" | "safe_stop_verifier_weak" => {
            "verification_failure"
        }
        "transport_error" => "transport_failure",
        "interrupted" => "interrupted",
        _ => "control_loop_failure",
    }
}

fn obligation(
    id: &str,
    status: &str,
    failure_domain: Option<&str>,
    detail: &str,
) -> TerminalObligationDiagnostic {
    TerminalObligationDiagnostic {
        id: id.to_string(),
        status: status.to_string(),
        failure_domain: failure_domain.map(str::to_string),
        detail: detail.to_string(),
    }
}

fn set_obligation(
    obligations: &mut [TerminalObligationDiagnostic],
    id: &str,
    status: &str,
    failure_domain: Option<&str>,
    detail: &str,
) {
    if let Some(obligation) = obligations.iter_mut().find(|o| o.id == id) {
        obligation.status = status.to_string();
        obligation.failure_domain = failure_domain.map(str::to_string);
        obligation.detail = detail.to_string();
    }
}

/// Replace absolute paths in a JSON string with `<path>` (opt-in via
/// `ANVIL_EVAL_SCRUB_PATHS=1`). Applied after `mask_payload_inplace`
/// (DR5 / design judgment #5).
///
/// Only matches paths where `/` is NOT immediately preceded by an
/// alphanumeric character, underscore, or dot so that relative paths like
/// `src/main.rs` are left intact (the `/` in that string is preceded by `c`).
pub fn scrub_absolute_paths(json: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;

    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // Capture group 1: the non-alphanumeric char (or empty at start) that
        // must precede the absolute path.  Group 2: the path itself.
        // Windows drive paths (C:\...) are also matched.
        Regex::new(r#"([^A-Za-z0-9_.]|^)((?:[A-Za-z]:[\\/]|/)(?:[^\s"\\,\[\]{}\r\n]+))"#)
            .expect("valid absolute path regex")
    });
    re.replace_all(json, |caps: &regex::Captures<'_>| {
        format!("{}<path>", &caps[1])
    })
    .into_owned()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn truncate_bytes(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    // Truncate on UTF-8 char boundary
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::case_retrieval::CaseScoreBreakdown;

    fn make_eval_record() -> EvalRecord {
        EvalRecord {
            schema_version: 1,
            session_id: "sess-001".to_string(),
            ts_ms: 1_700_000_000_000,
            task: "fix the bug".to_string(),
            model: "qwen3:14b".to_string(),
            mode: "Act".to_string(),
            tool_protocol: "native".to_string(),
            tool_calls: vec![ToolCallSummary {
                name: "Bash".to_string(),
                args_summary: "cargo build".to_string(),
            }],
            feedback_frame: None,
            active_precautions: vec![],
            anvil_score: None,
            changed_file_classes: ChangedFileClasses {
                test: 1,
                impl_files: 2,
                setup: 0,
            },
            verify_commands: vec!["cargo test".to_string()],
            case_retrieval_result: None,
            photon_eval: None,
            photon_canary: 0,
            auto_promote: None,
            terminal_diagnostics: Some(build_terminal_diagnostics(
                "done",
                &ChangedFileClasses {
                    test: 1,
                    impl_files: 2,
                    setup: 0,
                },
                1,
            )),
            final_outcome: "done".to_string(),
        }
    }

    #[test]
    fn build_eval_record_basic() {
        let rec = build_eval_record(
            "sess-001",
            12345,
            "fix the bug",
            "qwen3:14b",
            "Act",
            "native",
            &[ToolCallSummary {
                name: "Bash".to_string(),
                args_summary: "cargo build".to_string(),
            }],
            None,
            &[],
            None,
            ChangedFileClasses {
                test: 0,
                impl_files: 1,
                setup: 0,
            },
            &["cargo test".to_string()],
            None,
            None,
            None,
            "done",
        );
        assert_eq!(rec.schema_version, 1);
        assert_eq!(rec.session_id, "sess-001");
        assert_eq!(rec.ts_ms, 12345);
        assert_eq!(rec.model, "qwen3:14b");
        assert_eq!(rec.tool_calls.len(), 1);
        assert_eq!(rec.final_outcome, "done");
        assert_eq!(
            rec.terminal_diagnostics
                .as_ref()
                .map(|d| d.classification.as_str()),
            Some("success")
        );
    }

    #[test]
    fn terminal_diagnostics_classifies_issue_848_outcomes() {
        let changed = ChangedFileClasses {
            test: 1,
            impl_files: 1,
            setup: 0,
        };
        for (outcome, classification, failed_obligation, failure_domain) in [
            ("done", "success", None, None),
            (
                "missing_repo_edits",
                "model_output_failure",
                Some("repo_edit"),
                Some("model_output_failure"),
            ),
            (
                "safe_stop_verifier_missing",
                "verification_environment_failure",
                Some("verification_environment"),
                Some("verification_environment_failure"),
            ),
            (
                "repair_exhausted",
                "control_loop_failure",
                Some("repair_convergence"),
                Some("control_loop_failure"),
            ),
            (
                "tool_call_format_error",
                "model_output_failure",
                Some("model_output_format"),
                Some("model_output_failure"),
            ),
        ] {
            let diag = build_terminal_diagnostics(outcome, &changed, 1);
            assert_eq!(diag.outcome, outcome);
            assert_eq!(diag.classification, classification);
            if let Some(id) = failed_obligation {
                let obligation = diag
                    .obligations
                    .iter()
                    .find(|o| o.id == id)
                    .unwrap_or_else(|| panic!("{outcome} missing obligation {id}"));
                assert_eq!(obligation.status, "unsatisfied");
                assert_eq!(obligation.failure_domain.as_deref(), failure_domain);
            } else {
                assert!(
                    diag.obligations.iter().all(|o| o.status != "unsatisfied"),
                    "done should not contain an unsatisfied obligation: {:?}",
                    diag.obligations
                );
            }
        }
    }

    #[test]
    fn build_eval_record_masks_secrets_in_task() {
        let rec = build_eval_record(
            "sess-002",
            0,
            "use api_key=sk-proj-supersecret123 to fetch data",
            "model",
            "Act",
            "native",
            &[],
            None,
            &[],
            None,
            ChangedFileClasses {
                test: 0,
                impl_files: 0,
                setup: 0,
            },
            &[],
            None,
            None,
            None,
            "done",
        );
        assert!(
            !rec.task.contains("sk-proj-supersecret123"),
            "task: {}",
            rec.task
        );
    }

    #[test]
    fn build_eval_record_truncates_long_task() {
        let long_task = "x".repeat(MAX_EVAL_TASK_BYTES + 100);
        let rec = build_eval_record(
            "sess-003",
            0,
            &long_task,
            "model",
            "Act",
            "native",
            &[],
            None,
            &[],
            None,
            ChangedFileClasses {
                test: 0,
                impl_files: 0,
                setup: 0,
            },
            &[],
            None,
            None,
            None,
            "done",
        );
        assert!(rec.task.len() <= MAX_EVAL_TASK_BYTES + 10); // + truncation marker
    }

    #[test]
    fn build_eval_record_caps_precautions() {
        let precautions: Vec<EvalPrecautionSnapshot> = (0..20)
            .map(|i| EvalPrecautionSnapshot {
                id: format!("p{i}"),
                source: "manual".to_string(),
                severity: "medium".to_string(),
                status: "active".to_string(),
            })
            .collect();
        let rec = build_eval_record(
            "sess-004",
            0,
            "task",
            "model",
            "Act",
            "native",
            &[],
            None,
            &precautions,
            None,
            ChangedFileClasses {
                test: 0,
                impl_files: 0,
                setup: 0,
            },
            &[],
            None,
            None,
            None,
            "done",
        );
        assert!(rec.active_precautions.len() <= MAX_EVAL_PRECAUTIONS);
    }

    // Issue #604 Task 4.1: build_eval_record carries through auto_promote.
    #[test]
    fn build_eval_record_attaches_auto_promote_summary() {
        let summary = AutoPromoteOutcomeSummary {
            decision: "promoted".to_string(),
            skip_reason: None,
            summary_id: Some("anvil-case-aaaaaaaaaaaaaaaa".to_string()),
        };
        let rec = build_eval_record(
            "sess-604",
            0,
            "task",
            "model",
            "Act",
            "native",
            &[],
            None,
            &[],
            None,
            ChangedFileClasses {
                test: 0,
                impl_files: 0,
                setup: 0,
            },
            &[],
            None,
            None,
            Some(summary.clone()),
            "done",
        );
        let got = rec.auto_promote.expect("auto_promote present");
        assert_eq!(got, summary);
        // DR2-007: schema stays 3-field flat when serialized.
        let v = serde_json::to_value(&got).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 3, "3-field flat: {v}");
        assert_eq!(obj["decision"], "promoted");
        assert!(obj["skip_reason"].is_null());
        assert_eq!(obj["summary_id"], "anvil-case-aaaaaaaaaaaaaaaa");
    }

    // Issue #604 Task 4.1: when auto_promote is None, the field is omitted
    // from the JSON output entirely (backward compat with pre-#604 readers).
    #[test]
    fn eval_record_omits_auto_promote_when_none() {
        let mut rec = make_eval_record();
        rec.auto_promote = None;
        let json = serde_json::to_value(&rec).unwrap();
        assert!(
            json.get("auto_promote").is_none(),
            "auto_promote should be omitted when None: {json}",
        );
    }

    #[test]
    fn write_eval_record_produces_valid_json_line() {
        use std::io::BufRead;
        use tempfile::NamedTempFile;

        let tmpfile = NamedTempFile::new().unwrap();
        let path = tmpfile.path().to_path_buf();

        // Use a separate OnceLock-guarded logger just for test
        // (real EVAL_LOG_LOGGER might already be set; we write directly)
        {
            use std::io::Write;
            let rec = make_eval_record();
            let mut value = serde_json::to_value(&rec).unwrap();
            mask_payload_inplace(&mut value);
            let json = serde_json::to_string(&value).unwrap();
            assert!(json.len() < MAX_EVAL_LOG_RECORD_BYTES);

            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap();
            writeln!(file, "{json}").unwrap();
        }

        let file = std::fs::File::open(&path).unwrap();
        let mut lines = std::io::BufReader::new(file).lines();
        let line = lines.next().unwrap().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&line).expect("valid JSON");
        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["session_id"], "sess-001");
    }

    #[test]
    fn scrub_absolute_paths_replaces_unix_paths() {
        // Set env for scrub logic
        let json = r#"{"task":"/home/user/project/src/main.rs","ok":true}"#;
        let scrubbed = scrub_absolute_paths(json);
        assert!(!scrubbed.contains("/home/user"), "scrubbed: {scrubbed}");
        assert!(scrubbed.contains("<path>"), "scrubbed: {scrubbed}");
    }

    #[test]
    fn scrub_absolute_paths_leaves_relative_paths_intact() {
        let json = r#"{"file":"src/main.rs","count":3}"#;
        let scrubbed = scrub_absolute_paths(json);
        assert_eq!(scrubbed, json);
    }

    #[test]
    fn scrub_absolute_paths_produces_valid_json() {
        let json = r#"{"path":"/home/user/secret/file.rs","value":42}"#;
        let scrubbed = scrub_absolute_paths(json);
        serde_json::from_str::<serde_json::Value>(&scrubbed).expect("scrubbed is valid JSON");
    }

    #[test]
    fn anvil_score_summary_from_anvil_score() {
        let score = AnvilScore {
            build_passed: Some(true),
            tests_passed: Some(false),
            compile_errors_delta: Some(-2),
            test_failures_delta: None,
            compile_error_count: Some(0),
            test_failure_count: Some(3),
            implementation_files_changed: Some(1),
            test_files_changed: Some(0),
            setup_files_changed: None,
            unsafe_actions_blocked: 0,
            consecutive_no_progress_turns: 0,
            user_visible_artifact: true,
        };
        let summary = AnvilScoreSummary::from(&score);
        assert_eq!(summary.build_passed, Some(true));
        assert_eq!(summary.tests_passed, Some(false));
        assert_eq!(summary.compile_errors_delta, Some(-2));
        assert_eq!(summary.test_failure_count, Some(3));
        assert!(summary.user_visible_artifact);
    }

    #[test]
    fn case_retrieval_summary_serializes_scores() {
        let summary = CaseRetrievalSummary {
            selected: 2,
            scores: vec![CaseScoreBreakdown {
                case_id: "case_abc".to_string(),
                task: 0.5,
                semantic: 0.0,
                stack: 0.1,
                repo: 0.2,
                files: 0.1,
                kind: 0.05,
                precautions: 0.05,
                total: 0.5,
            }],
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["selected"], 2);
        assert_eq!(json["scores"][0]["case_id"], "case_abc");
    }

    #[test]
    fn eval_precaution_snapshot_from_precaution() {
        use crate::session::precaution::{
            Precaution, PrecautionSource, PrecautionStatus, Severity,
        };
        let p = Precaution {
            id: "p001".to_string(),
            source: PrecautionSource::BuildFailure,
            severity: Severity::High,
            text: "build failed".to_string(),
            applies_to: vec![],
            status: PrecautionStatus::Active,
            retired_reason: None,
        };
        let snap = EvalPrecautionSnapshot::from(&p);
        assert_eq!(snap.id, "p001");
        assert_eq!(snap.source, "build_failure");
        assert_eq!(snap.severity, "high");
        assert_eq!(snap.status, "active");
    }

    #[test]
    fn truncate_bytes_trims_at_char_boundary() {
        let s = "hello world extra stuff";
        let truncated = truncate_bytes(s, 5);
        assert!(truncated.len() <= 10); // "hello" + "…"
        // Multi-byte: Japanese chars are 3 bytes each
        let jp = "日本語テスト";
        let truncated_jp = truncate_bytes(jp, 7);
        // 7 bytes: 日(3) + 本(3) = 6 < 7, テ(3) would overflow → truncate at 6
        assert!(std::str::from_utf8(truncated_jp.trim_end_matches('…').as_bytes()).is_ok());
    }
}
