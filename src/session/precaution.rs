//! Precaution: structured runtime constraint for next generation/edit/verification.
//!
//! Stored as `WorkingMemory.active_precautions` (Issue #2 / Epic A #451).
//!
//! IMPORTANT: must be constructed via `WorkingMemory::add_precaution` to apply
//! `mask_secrets` / `truncate_entry` / canonical id. Direct struct-literal
//! construction is technically possible within the same crate but bypasses the
//! canonicalization pipeline. After session.json is loaded, callers MUST invoke
//! `WorkingMemory::sanitize_active_precautions_after_load` to defend against
//! manually edited / future-version session.json (design judgment #14).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Structured runtime constraint that gets injected into the LLM prompt.
///
/// Field-level note (design judgment #10, DR4-005):
/// `Debug` is implemented manually so that `text` itself never lands in
/// stderr/log output (it may carry masked-but-still-sensitive content);
/// only `text_len` is shown.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Precaution {
    pub id: String,
    pub source: PrecautionSource,
    pub severity: Severity,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applies_to: Vec<PathBuf>,
    pub status: PrecautionStatus,
    /// Why this precaution was retired (only populated when status = Retired).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_reason: Option<RetiredReason>,
}

impl std::fmt::Debug for Precaution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Precaution")
            .field("id", &self.id)
            .field("source", &self.source)
            .field("severity", &self.severity)
            .field("text_len", &self.text.chars().count())
            .field("applies_to", &self.applies_to)
            .field("status", &self.status)
            .field("retired_reason", &self.retired_reason)
            .finish()
    }
}

/// Origin of a precaution — manual (user) or one of several automated paths.
///
/// Default is `Manual` (DR1-008): we deliberately do not default to `Unknown`
/// because that would let `..Default::default()` masquerade as a forward-compat
/// fallback. `Unknown` is reserved for `#[serde(other)]` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PrecautionSource {
    UserRequirement,
    PlanConstraint,
    BuildFailure,
    TestFailure,
    ToolFailure,
    NoProgress,
    SafetyPolicy,
    /// Default for `..Default::default()` use sites is intentionally `Manual`,
    /// not `Unknown`, to avoid masquerading deliberate user input as a
    /// forward-compat fallback. (DR1-008)
    #[default]
    Manual,
    /// Fallback for forward compatibility (unknown variant in old session.json).
    /// Used by `#[serde(other)]` only — never produced by `Default::default()`.
    #[serde(other)]
    Unknown,
}

/// Lifecycle state of a precaution.
///
/// `Unknown` is a forward-compat fallback (DR4-002). After session load,
/// `Unknown` is sanitized to `Retired(Unknown)` and never prompt-injected as
/// `Active`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PrecautionStatus {
    #[default]
    Active,
    Resolved,
    Retired,
    /// Forward-compat fallback for unknown status values in session.json.
    /// Unknown must never be prompt-injected as Active.
    #[serde(other)]
    Unknown,
}

/// Severity of a precaution. Default is `Medium`.
///
/// `Unknown` is a forward-compat fallback (CB-003): a session.json containing
/// a future severity value must still deserialize so that `--resume` does not
/// fail. `Unknown` is normalized to `Medium` by the load-side sanitizer
/// (`canonicalize_loaded_precaution`) so the prompt label is always known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    High,
    #[default]
    Medium,
    Low,
    /// Forward-compat fallback for unknown severity values in session.json.
    /// Sanitized to `Medium` during canonicalization, never produced by
    /// `Default::default()` (parallel to `PrecautionSource::Unknown`, DR1-008).
    #[serde(other)]
    Unknown,
}

impl Severity {
    /// Stable snake_case label for prompt formatting and logs.
    /// Matches `#[serde(rename_all = "snake_case")]` so prompt and session.json
    /// representations stay aligned (DR1-005). `Unknown` is rendered as
    /// `"unknown"` so logs / prompts can still distinguish it from `Medium`
    /// when the sanitizer has not yet run; in normal flow the load sanitizer
    /// downgrades `Unknown` to `Medium` before the prompt is built.
    pub fn as_label(&self) -> &'static str {
        match self {
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
            Severity::Unknown => "unknown",
        }
    }
}

/// Reason a precaution was retired. Default is `UserRetired` (DR1-011).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RetiredReason {
    /// User issued `/precautions retire <id>`. Terminal — same key cannot be re-Activated.
    #[default]
    UserRetired,
    /// Capacity-based FIFO eviction. Same key can be re-Activated.
    CapacityEvicted,
    /// Forward-compat fallback for unknown variants in old session.json.
    #[serde(other)]
    Unknown,
}

/// Outcome of `WorkingMemory::add_precaution`.
///
/// `Truncated` indicates the precaution was added but its `text` was truncated
/// to fit `MAX_PRECAUTION_TEXT`. There is intentionally no `RejectedTooLong`
/// variant: storage layer always truncates and stores. Hard reject (if needed)
/// belongs to the CLI wrapper (#455) — see design judgment #6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddPrecautionOutcome {
    Added,
    DuplicateIgnored,
    /// Text exceeded MAX_PRECAUTION_TEXT and was truncated, but Added.
    Truncated,
}
