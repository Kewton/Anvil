//! Case → Photon seed bridge (Issue #593, Phase A).
//!
//! Converts successful `CaseRecord` entries (#462) into photon `ActionSummary`
//! v0.2 format and persists a dedup audit log. Phase A scope:
//!
//! - Manual CLI (`anvil sessions photon-promote`) + dry-run table + local JSONL
//!   output. HTTP POST to the photon sidecar is Phase B (separate Issue).
//! - Quality Gate filters with 6 deterministic skip reasons (Issue #593 design).
//!
//! Layering (DR3-002): this module **must not** import from `crate::photon::*`.
//! photon schema-compatible types (`ActionSummary` / `Fact` / ...) are defined
//! locally here, mirroring the `PhotonEvalSummary` precedent in `eval_log.rs`.

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::logging::mask_payload_inplace;
use crate::session::case_record::{CaseRecord, RepoFingerprint};
use crate::session::feedback::{FeedbackKind, mask_secrets};

// ---------------------------------------------------------------------------
// SSOT constants
// ---------------------------------------------------------------------------

pub const MAX_ACTION_SUMMARY_BYTES: usize = 16 * 1024;
pub const FRESHNESS_THRESHOLD_DAYS: u64 = 30;
pub const MAX_PHOTON_PROMOTE_LOG_BYTES: u64 = 1024 * 1024;
pub const ACTION_SUMMARY_SCHEMA_VERSION: &str = "action-memory.v0.2";
pub const SUMMARY_ID_PREFIX: &str = "anvil-case-";
pub const PROVENANCE_SOURCE: &str = "anvil_case_record";

/// FeedbackKind values that count as failure signals when building the
/// `avoid` section of an `ActionSummary`. Kinds outside this slice (e.g.
/// `BuildPass` / `TestPass`) are skipped on the avoid path.
const FAILURE_KINDS: &[FeedbackKind] = &[
    FeedbackKind::CompileError,
    FeedbackKind::TypeError,
    FeedbackKind::LintFailure,
    FeedbackKind::TestFailure,
    FeedbackKind::UnsafeCommandBlocked,
    FeedbackKind::EditFailure,
    FeedbackKind::NoRepoProgress,
    FeedbackKind::ToolProtocolFailure,
    FeedbackKind::UnknownFailure,
];

// ---------------------------------------------------------------------------
// Types (photon ActionSummary v0.2 compatible)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionSummary {
    pub schema_version: String,
    pub summary_id: String,
    pub repo_id: String,
    pub task_signature: String,
    pub facts: Vec<Fact>,
    pub avoid: Vec<Avoid>,
    pub next_hints: Vec<Hint>,
    #[serde(rename = "_provenance", skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    pub text: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Avoid {
    pub text: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hint {
    pub kind: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub source: String,
    pub case_id: String,
    pub session_id: String,
    pub anvil_version: String,
    pub extracted_at: String,
    pub confidence_prior: f32,
    pub verifier_active: bool,
}

// Phase B: `SummarizeRequest { summary: ActionSummary }` will be added here when
// `PhotonClient::summarize()` is implemented (DR1-004 / DR3-002 maintained:
// definition stays in session layer).

// ---------------------------------------------------------------------------
// Outcome / log types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BridgeOutcome {
    pub promoted: Vec<PromotedEntry>,
    pub skipped: Vec<SkippedEntry>,
}

#[derive(Debug, Clone)]
pub struct PromotedEntry {
    pub case_id: String,
    pub summary_id: String,
    pub confidence_prior: f32,
    pub byte_size: usize,
}

#[derive(Debug, Clone)]
pub struct SkippedEntry {
    pub case_id: String,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    AlreadyPromoted,
    NotSuccessState,
    EmptyLanguageStack,
    EmptyTaskSignature,
    StaleCase,
    Oversize,
}

impl SkipReason {
    /// snake_case tag string (mirrors `#[serde(rename_all = "snake_case")]`
    /// exactly so callers can embed the tag in log payloads without round-trip).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyPromoted => "already_promoted",
            Self::NotSuccessState => "not_success_state",
            Self::EmptyLanguageStack => "empty_language_stack",
            Self::EmptyTaskSignature => "empty_task_signature",
            Self::StaleCase => "stale_case",
            Self::Oversize => "oversize",
        }
    }
}

/// One line in `state_root/photon-promote-log.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromoteLogEntry {
    pub case_id: String,
    pub summary_id: String,
    pub extracted_at: String,
    pub outcome: String, // "promoted" | "skipped"
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Pure functions
// ---------------------------------------------------------------------------

use crate::session::anvil_score::AnvilScore;

/// SSOT for "was a verifier (cargo build / cargo test) active for this turn?".
/// Used by all 4 promotion-related call sites (DR1-002).
pub fn is_verifier_active(score: &AnvilScore) -> bool {
    score.build_passed.is_some() || score.tests_passed.is_some()
}

/// Defensive re-evaluation of CaseRecord persistence success criteria.
/// CaseRecord is already persisted only on success turns; this is a second
/// layer (DR1-001 / design judgment #4.5).
pub fn is_eligible_for_promotion(case: &CaseRecord) -> bool {
    let score = &case.outcome_score;
    if is_verifier_active(score) {
        score.build_passed == Some(true)
            && score.tests_passed == Some(true)
            && score.user_visible_artifact
            && score.consecutive_no_progress_turns == 0
    } else {
        // verifier-less fallback: use AnvilScore-persisted derived values.
        score.user_visible_artifact
            && score.unsafe_actions_blocked == 0
            && score.consecutive_no_progress_turns == 0
    }
}

/// 3-tier confidence prior derivation (design judgment #2 / case B).
pub fn derive_confidence_prior(score: &AnvilScore, verifier_active: bool) -> f32 {
    if verifier_active {
        let both_passed = score.build_passed == Some(true) && score.tests_passed == Some(true);
        match (both_passed, score.user_visible_artifact) {
            (true, true) => 1.0,
            (true, false) => 0.8,
            _ => 0.6,
        }
    } else {
        0.6
    }
}

/// 6-filter Quality Gate (design §5.4).
///
/// `now_unix` is injected so freshness checks are deterministic in tests.
pub fn check_quality_gate(
    case: &CaseRecord,
    already_promoted_ids: &HashSet<String>,
    now_unix: u64,
) -> Result<(), SkipReason> {
    if !is_eligible_for_promotion(case) {
        return Err(SkipReason::NotSuccessState);
    }
    if already_promoted_ids.contains(&case.case_id) {
        return Err(SkipReason::AlreadyPromoted);
    }
    if case.language_stack.is_empty() {
        return Err(SkipReason::EmptyLanguageStack);
    }
    if case.task_signature.trim().is_empty() {
        return Err(SkipReason::EmptyTaskSignature);
    }
    let age_secs = now_unix.saturating_sub(case.created_at);
    if age_secs > FRESHNESS_THRESHOLD_DAYS * 86_400 {
        return Err(SkipReason::StaleCase);
    }
    // Oversize is checked post-serialize by the caller.
    Ok(())
}

/// Convert a `CaseRecord` to an `ActionSummary` (pure, no I/O).
///
/// `session_id` falls back to `"unknown"` when the caller did not specify
/// `--session` (CaseRecord does not store the session id).
/// `extracted_at` is RFC3339 and must be injected by the caller so that all
/// summaries in one CLI run share the same timestamp (DR1-007).
pub fn convert_case_to_action_summary(
    case: &CaseRecord,
    session_id: &str,
    extracted_at: &str,
) -> ActionSummary {
    let verifier_active = is_verifier_active(&case.outcome_score);
    let confidence_prior = derive_confidence_prior(&case.outcome_score, verifier_active);

    let repo_id = build_repo_id(&case.repo_fingerprint);

    let facts = build_facts(case, confidence_prior);
    let avoid = build_avoid(case, confidence_prior);
    let next_hints = case
        .verify_commands
        .iter()
        .map(|cmd| Hint {
            kind: "verify".to_string(),
            target: mask_secrets(cmd),
        })
        .collect();

    let provenance = Provenance {
        source: PROVENANCE_SOURCE.to_string(),
        case_id: case.case_id.clone(),
        session_id: session_id.to_string(),
        anvil_version: env!("CARGO_PKG_VERSION").to_string(),
        extracted_at: extracted_at.to_string(),
        confidence_prior,
        verifier_active,
    };

    let summary = ActionSummary {
        schema_version: ACTION_SUMMARY_SCHEMA_VERSION.to_string(),
        summary_id: format!("{SUMMARY_ID_PREFIX}{}", case.case_id),
        repo_id,
        task_signature: mask_secrets(&case.task_signature),
        facts,
        avoid,
        next_hints,
        provenance: Some(provenance),
    };
    apply_defensive_mask(summary)
}

/// 3-tier fallback for repo identification (design §5.3).
fn build_repo_id(fingerprint: &RepoFingerprint) -> String {
    if let Some(remote) = &fingerprint.git_remote {
        let masked = mask_secrets(remote);
        if !masked.trim().is_empty() {
            return masked;
        }
    }
    let workspace = fingerprint.workspace_key.trim();
    if !workspace.is_empty() {
        return format!("workspace:{}", mask_secrets(workspace));
    }
    format!("langhash:{}", fingerprint.language_stack_hash)
}

/// Build the `facts` section. Includes language stack, verify commands, and
/// successful precautions (positive signals).
fn build_facts(case: &CaseRecord, confidence_prior: f32) -> Vec<Fact> {
    let mut out = Vec::new();
    if !case.language_stack.is_empty() {
        let langs = case
            .language_stack
            .iter()
            .map(|s| mask_secrets(s))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(Fact {
            text: format!("language_stack: {langs}"),
            confidence: confidence_prior,
        });
    }
    for snap in &case.successful_precautions {
        out.push(Fact {
            text: format!("precaution: {}", mask_secrets(&snap.text)),
            confidence: confidence_prior,
        });
    }
    out
}

/// Build the `avoid` section from `initial_feedback`, filtered to failure
/// kinds. Each `Avoid` entry uses the FeedbackKind tag string.
fn build_avoid(case: &CaseRecord, confidence_prior: f32) -> Vec<Avoid> {
    case.initial_feedback
        .iter()
        .filter(|kind| FAILURE_KINDS.contains(kind))
        .map(|kind| Avoid {
            text: format!("initial_feedback: {}", kind.as_str()),
            confidence: confidence_prior,
        })
        .collect()
}

/// Apply `mask_payload_inplace` as a defensive second layer. If the round-trip
/// fails (should not happen with our own types), the unmasked summary is
/// returned unchanged.
fn apply_defensive_mask(summary: ActionSummary) -> ActionSummary {
    let Ok(mut v) = serde_json::to_value(&summary) else {
        return summary;
    };
    mask_payload_inplace(&mut v);
    serde_json::from_value(v).unwrap_or(summary)
}

// ---------------------------------------------------------------------------
// I/O functions
// ---------------------------------------------------------------------------

/// Construct the dedup set of already-promoted `case_id`s by scanning
/// `state_root/photon-promote-log.jsonl`. Returns `Ok(empty)` if the file is
/// missing or oversize (with stderr warning).
pub fn read_promote_log(state_root: &Path) -> Result<HashSet<String>, String> {
    let path = state_root.join("photon-promote-log.jsonl");
    let meta = match path.symlink_metadata() {
        Ok(m) => m,
        Err(_) => return Ok(HashSet::new()),
    };
    if meta.file_type().is_symlink() {
        return Err(format!(
            "photon-promote-log.jsonl is a symlink: {}",
            path.display()
        ));
    }
    if !meta.is_file() {
        return Err(format!(
            "photon-promote-log.jsonl is not a regular file: {}",
            path.display()
        ));
    }
    if meta.len() > MAX_PHOTON_PROMOTE_LOG_BYTES {
        eprintln!(
            "warning: photon-promote-log.jsonl size ({} bytes) exceeds {} bytes; \
             dedup set built from current contents anyway",
            meta.len(),
            MAX_PHOTON_PROMOTE_LOG_BYTES
        );
    }
    let file = std::fs::File::open(&path)
        .map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    let reader = BufReader::new(file);
    let mut promoted = HashSet::new();
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<PromoteLogEntry>(&line) else {
            continue;
        };
        if entry.outcome == "promoted" {
            promoted.insert(entry.case_id);
        }
    }
    Ok(promoted)
}

/// Append a `PromoteLogEntry` to `state_root/photon-promote-log.jsonl`.
/// Sets 0o600 on first creation (cfg(unix)). On size overflow emits a warning
/// but continues appending (rotation is Phase B, DR1-006).
pub fn append_promote_log_entry(state_root: &Path, entry: &PromoteLogEntry) -> Result<(), String> {
    std::fs::create_dir_all(state_root)
        .map_err(|e| format!("failed to create state_root {}: {e}", state_root.display()))?;
    let path = state_root.join("photon-promote-log.jsonl");
    // Reject symlinks before opening for append (defence in depth).
    if let Ok(meta) = path.symlink_metadata()
        && meta.file_type().is_symlink()
    {
        return Err(format!(
            "photon-promote-log.jsonl is a symlink: {}",
            path.display()
        ));
    }
    rotate_promote_log_if_needed(state_root)?;

    let needs_chmod = !path.exists();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    let mut json =
        serde_json::to_string(entry).map_err(|e| format!("serialize PromoteLogEntry: {e}"))?;
    json.push('\n');
    file.write_all(json.as_bytes())
        .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        if needs_chmod {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = needs_chmod;
    }
    Ok(())
}

/// Phase A stub: emits a warning if the dedup log exceeds the size cap but
/// does not evict old lines. Phase B will implement per-entry LRU eviction.
pub fn rotate_promote_log_if_needed(state_root: &Path) -> Result<(), String> {
    let path = state_root.join("photon-promote-log.jsonl");
    let Ok(meta) = path.symlink_metadata() else {
        return Ok(());
    };
    if meta.file_type().is_symlink() {
        return Err(format!(
            "photon-promote-log.jsonl is a symlink: {}",
            path.display()
        ));
    }
    if meta.len() > MAX_PHOTON_PROMOTE_LOG_BYTES {
        eprintln!(
            "warning: photon-promote-log.jsonl is {} bytes (> {} cap); \
             rotation is Phase B, append continues",
            meta.len(),
            MAX_PHOTON_PROMOTE_LOG_BYTES
        );
    }
    Ok(())
}

/// Write an ActionSummary slice as JSONL to `path` (one summary per line).
/// New file only (`create_new(true)`), 0o600 on unix, symlink rejected.
pub fn write_jsonl_output(path: &Path, summaries: &[ActionSummary]) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        return Err(format!(
            "output parent directory does not exist: {}",
            parent.display()
        ));
    }
    if let Ok(meta) = path.symlink_metadata() {
        if meta.file_type().is_symlink() {
            return Err(format!("output path is a symlink: {}", path.display()));
        }
        return Err(format!("output path already exists: {}", path.display()));
    }
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(path)
        .map_err(|e| format!("failed to create output {}: {e}", path.display()))?;
    for summary in summaries {
        let mut json =
            serde_json::to_string(summary).map_err(|e| format!("serialize ActionSummary: {e}"))?;
        json.push('\n');
        file.write_all(json.as_bytes())
            .map_err(|e| format!("failed to write output {}: {e}", path.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Time helpers (RFC3339 builder, no chrono dependency)
// ---------------------------------------------------------------------------

/// Build a fixed-format RFC3339 timestamp string `YYYY-MM-DDTHH:MM:SSZ` from
/// a Unix epoch seconds value. Pure helper to keep the CLI handler testable
/// (DR1-007: same value across all summaries in one run).
pub fn format_rfc3339_utc(unix_secs: u64) -> String {
    // Days since 1970-01-01 (Thursday). Standard civil_from_days port.
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let sec = secs_of_day % 60;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, minute, sec
    )
}

/// Howard Hinnant's `civil_from_days` algorithm — converts days since
/// 1970-01-01 (Unix epoch) to (year, month, day) in the Gregorian calendar.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m as u32, d as u32)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::anvil_score::AnvilScore;
    use crate::session::case_record::{CaseRecord, RepoFingerprint};

    fn baseline_score() -> AnvilScore {
        AnvilScore {
            build_passed: Some(true),
            tests_passed: Some(true),
            compile_errors_delta: Some(0),
            test_failures_delta: Some(0),
            compile_error_count: Some(0),
            test_failure_count: Some(0),
            implementation_files_changed: Some(1),
            test_files_changed: Some(1),
            setup_files_changed: Some(0),
            unsafe_actions_blocked: 0,
            consecutive_no_progress_turns: 0,
            user_visible_artifact: true,
        }
    }

    fn baseline_case(id: &str) -> CaseRecord {
        CaseRecord {
            case_id: id.to_string(),
            created_at: 1_700_000_000,
            repo_fingerprint: RepoFingerprint {
                workspace_key: "ws-key".to_string(),
                git_remote: Some("https://github.com/o/r.git".to_string()),
                git_head_branch: Some("main".to_string()),
                language_stack_hash: "deadbeefdeadbeef".to_string(),
            },
            task_signature: "fix bug".to_string(),
            language_stack: vec!["rust".to_string()],
            initial_feedback: vec![FeedbackKind::CompileError],
            successful_precautions: vec![],
            changed_files_summary: vec![],
            verify_commands: vec!["cargo test".to_string()],
            outcome_score: baseline_score(),
        }
    }

    // --- is_verifier_active --------------------------------------------------

    #[test]
    fn is_verifier_active_true_when_build_passed_some() {
        let mut s = baseline_score();
        s.tests_passed = None;
        assert!(is_verifier_active(&s));
    }

    #[test]
    fn is_verifier_active_true_when_tests_passed_some() {
        let mut s = baseline_score();
        s.build_passed = None;
        assert!(is_verifier_active(&s));
    }

    #[test]
    fn is_verifier_active_false_when_both_none() {
        let mut s = baseline_score();
        s.build_passed = None;
        s.tests_passed = None;
        assert!(!is_verifier_active(&s));
    }

    // --- is_eligible_for_promotion ------------------------------------------

    #[test]
    fn is_eligible_verifier_passes_and_artifact() {
        let c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        assert!(is_eligible_for_promotion(&c));
    }

    #[test]
    fn is_eligible_verifier_build_failed() {
        let mut c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        c.outcome_score.build_passed = Some(false);
        assert!(!is_eligible_for_promotion(&c));
    }

    #[test]
    fn is_eligible_verifier_tests_failed() {
        let mut c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        c.outcome_score.tests_passed = Some(false);
        assert!(!is_eligible_for_promotion(&c));
    }

    #[test]
    fn is_eligible_no_progress_blocks() {
        let mut c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        c.outcome_score.consecutive_no_progress_turns = 1;
        assert!(!is_eligible_for_promotion(&c));
    }

    #[test]
    fn is_eligible_verifier_less_success() {
        let mut c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        c.outcome_score.build_passed = None;
        c.outcome_score.tests_passed = None;
        c.outcome_score.user_visible_artifact = true;
        c.outcome_score.unsafe_actions_blocked = 0;
        assert!(is_eligible_for_promotion(&c));
    }

    #[test]
    fn is_eligible_verifier_less_unsafe_blocks_fails() {
        let mut c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        c.outcome_score.build_passed = None;
        c.outcome_score.tests_passed = None;
        c.outcome_score.unsafe_actions_blocked = 1;
        assert!(!is_eligible_for_promotion(&c));
    }

    #[test]
    fn is_eligible_verifier_less_no_artifact_fails() {
        let mut c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        c.outcome_score.build_passed = None;
        c.outcome_score.tests_passed = None;
        c.outcome_score.user_visible_artifact = false;
        assert!(!is_eligible_for_promotion(&c));
    }

    // --- derive_confidence_prior --------------------------------------------

    #[test]
    fn confidence_full_passes_and_artifact() {
        let s = baseline_score();
        assert_eq!(derive_confidence_prior(&s, true), 1.0);
    }

    #[test]
    fn confidence_passes_no_artifact() {
        let mut s = baseline_score();
        s.user_visible_artifact = false;
        assert_eq!(derive_confidence_prior(&s, true), 0.8);
    }

    #[test]
    fn confidence_verifier_active_but_failed() {
        let mut s = baseline_score();
        s.tests_passed = Some(false);
        assert_eq!(derive_confidence_prior(&s, true), 0.6);
    }

    #[test]
    fn confidence_verifier_inactive() {
        let s = baseline_score();
        assert_eq!(derive_confidence_prior(&s, false), 0.6);
    }

    // --- check_quality_gate --------------------------------------------------

    #[test]
    fn quality_gate_ok_for_eligible_case() {
        let c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        let promoted: HashSet<String> = HashSet::new();
        let r = check_quality_gate(&c, &promoted, c.created_at + 1);
        assert!(r.is_ok());
    }

    #[test]
    fn quality_gate_already_promoted() {
        let c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        let mut promoted = HashSet::new();
        promoted.insert(c.case_id.clone());
        assert_eq!(
            check_quality_gate(&c, &promoted, c.created_at + 1),
            Err(SkipReason::AlreadyPromoted)
        );
    }

    #[test]
    fn quality_gate_not_success_state() {
        let mut c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        c.outcome_score.build_passed = Some(false);
        assert_eq!(
            check_quality_gate(&c, &HashSet::new(), c.created_at + 1),
            Err(SkipReason::NotSuccessState)
        );
    }

    #[test]
    fn quality_gate_empty_language_stack() {
        let mut c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        c.language_stack = vec![];
        assert_eq!(
            check_quality_gate(&c, &HashSet::new(), c.created_at + 1),
            Err(SkipReason::EmptyLanguageStack)
        );
    }

    #[test]
    fn quality_gate_empty_task_signature() {
        let mut c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        c.task_signature = "   ".to_string();
        assert_eq!(
            check_quality_gate(&c, &HashSet::new(), c.created_at + 1),
            Err(SkipReason::EmptyTaskSignature)
        );
    }

    #[test]
    fn quality_gate_stale_case() {
        let c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        let now = c.created_at + (FRESHNESS_THRESHOLD_DAYS + 1) * 86_400;
        assert_eq!(
            check_quality_gate(&c, &HashSet::new(), now),
            Err(SkipReason::StaleCase)
        );
    }

    #[test]
    fn quality_gate_fresh_boundary_inclusive() {
        let c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        // exactly FRESHNESS_THRESHOLD_DAYS old is fresh, +1 second is stale.
        let edge = c.created_at + FRESHNESS_THRESHOLD_DAYS * 86_400;
        assert!(check_quality_gate(&c, &HashSet::new(), edge).is_ok());
    }

    // --- convert_case_to_action_summary --------------------------------------

    #[test]
    fn convert_populates_schema_and_summary_id() {
        let c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        let s = convert_case_to_action_summary(&c, "sess-1", "2026-05-16T00:00:00Z");
        assert_eq!(s.schema_version, ACTION_SUMMARY_SCHEMA_VERSION);
        assert!(s.summary_id.starts_with(SUMMARY_ID_PREFIX));
        assert!(s.summary_id.ends_with(&c.case_id));
        assert_eq!(s.repo_id, "https://github.com/o/r.git");
    }

    #[test]
    fn convert_emits_provenance_with_verifier_active_true() {
        let c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        let s = convert_case_to_action_summary(&c, "sess-1", "2026-05-16T00:00:00Z");
        let prov = s.provenance.unwrap();
        assert_eq!(prov.source, PROVENANCE_SOURCE);
        assert_eq!(prov.session_id, "sess-1");
        assert!(prov.verifier_active);
        assert_eq!(prov.confidence_prior, 1.0);
    }

    #[test]
    fn convert_verifier_less_path_uses_06_confidence() {
        let mut c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        c.outcome_score.build_passed = None;
        c.outcome_score.tests_passed = None;
        let s = convert_case_to_action_summary(&c, "sess-1", "2026-05-16T00:00:00Z");
        let prov = s.provenance.unwrap();
        assert!(!prov.verifier_active);
        assert_eq!(prov.confidence_prior, 0.6);
    }

    // --- build_repo_id (3-tier fallback) -------------------------------------

    #[test]
    fn repo_id_uses_git_remote_when_present() {
        let fp = RepoFingerprint {
            workspace_key: "ws".into(),
            git_remote: Some("https://github.com/o/r.git".into()),
            git_head_branch: None,
            language_stack_hash: "deadbeefdeadbeef".into(),
        };
        assert_eq!(build_repo_id(&fp), "https://github.com/o/r.git");
    }

    #[test]
    fn repo_id_falls_back_to_workspace_key() {
        let fp = RepoFingerprint {
            workspace_key: "ws-key".into(),
            git_remote: None,
            git_head_branch: None,
            language_stack_hash: "deadbeefdeadbeef".into(),
        };
        assert_eq!(build_repo_id(&fp), "workspace:ws-key");
    }

    #[test]
    fn repo_id_falls_back_to_langhash() {
        let fp = RepoFingerprint {
            workspace_key: String::new(),
            git_remote: None,
            git_head_branch: None,
            language_stack_hash: "deadbeefdeadbeef".into(),
        };
        assert_eq!(build_repo_id(&fp), "langhash:deadbeefdeadbeef");
    }

    // --- apply_defensive_mask -----------------------------------------------

    #[test]
    fn apply_defensive_mask_masks_inline_token() {
        let mut c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        c.task_signature = "calling api_key=sk_live_supersecretvalue please".into();
        let s = convert_case_to_action_summary(&c, "sess-1", "2026-05-16T00:00:00Z");
        assert!(
            !s.task_signature.contains("sk_live_supersecretvalue"),
            "task_signature must be masked, got: {}",
            s.task_signature
        );
    }

    #[test]
    fn provenance_keys_are_not_secret_like() {
        // CB-10 supporting check: provenance field names must not match
        // is_secret_like_key, otherwise mask_payload_inplace would erase them.
        let keys = [
            "source",
            "case_id",
            "session_id",
            "anvil_version",
            "extracted_at",
            "confidence_prior",
            "verifier_active",
        ];
        for k in keys {
            assert!(
                !crate::logging::is_secret_like_key(k),
                "provenance key {k} must not be secret-like"
            );
        }
    }

    // --- I/O round-trip ------------------------------------------------------

    #[test]
    fn promote_log_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let entry = PromoteLogEntry {
            case_id: "case_aaaaaaaaaaaaaaaaaaaaaaaa".into(),
            summary_id: "anvil-case-case_aaaaaaaaaaaaaaaaaaaaaaaa".into(),
            extracted_at: "2026-05-16T00:00:00Z".into(),
            outcome: "promoted".into(),
            reason: None,
        };
        append_promote_log_entry(tmp.path(), &entry).unwrap();
        let promoted = read_promote_log(tmp.path()).unwrap();
        assert!(promoted.contains("case_aaaaaaaaaaaaaaaaaaaaaaaa"));
    }

    #[test]
    fn promote_log_skipped_outcome_not_in_dedup() {
        let tmp = tempfile::tempdir().unwrap();
        let entry = PromoteLogEntry {
            case_id: "case_aaaaaaaaaaaaaaaaaaaaaaaa".into(),
            summary_id: "anvil-case-case_aaaaaaaaaaaaaaaaaaaaaaaa".into(),
            extracted_at: "2026-05-16T00:00:00Z".into(),
            outcome: "skipped".into(),
            reason: Some("stale_case".into()),
        };
        append_promote_log_entry(tmp.path(), &entry).unwrap();
        let promoted = read_promote_log(tmp.path()).unwrap();
        assert!(promoted.is_empty());
    }

    #[test]
    fn read_promote_log_empty_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let promoted = read_promote_log(tmp.path()).unwrap();
        assert!(promoted.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn write_jsonl_output_creates_with_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out.jsonl");
        let c = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        let s = convert_case_to_action_summary(&c, "sess-1", "2026-05-16T00:00:00Z");
        write_jsonl_output(&out, &[s]).unwrap();
        let mode = std::fs::metadata(&out).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600 perms, got {mode:o}");
    }

    #[test]
    fn write_jsonl_output_rejects_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out.jsonl");
        std::fs::write(&out, b"existing").unwrap();
        let err = write_jsonl_output(&out, &[]).unwrap_err();
        assert!(err.contains("already exists"));
    }

    #[cfg(unix)]
    #[test]
    fn write_jsonl_output_rejects_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("real.jsonl");
        std::fs::write(&target, b"x").unwrap();
        let link = tmp.path().join("link.jsonl");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = write_jsonl_output(&link, &[]).unwrap_err();
        assert!(err.contains("symlink"));
    }

    #[test]
    fn write_jsonl_output_one_line_per_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out.jsonl");
        let c1 = baseline_case("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        let c2 = baseline_case("case_bbbbbbbbbbbbbbbbbbbbbbbb");
        let s1 = convert_case_to_action_summary(&c1, "s", "t");
        let s2 = convert_case_to_action_summary(&c2, "s", "t");
        write_jsonl_output(&out, &[s1, s2]).unwrap();
        let body = std::fs::read_to_string(&out).unwrap();
        assert_eq!(body.lines().count(), 2);
    }

    // --- RFC3339 helper ------------------------------------------------------

    #[test]
    fn rfc3339_unix_epoch_zero() {
        assert_eq!(format_rfc3339_utc(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn rfc3339_known_value() {
        // 2024-01-01T00:00:00Z == 1_704_067_200
        assert_eq!(format_rfc3339_utc(1_704_067_200), "2024-01-01T00:00:00Z");
    }
}
