//! Patch-admission helpers for verifier repair.
//!
//! This module owns pure checks that do not need the actor loop or filesystem.
//! `turn.rs` can map the typed rejection into its legacy `ValidationFailure`
//! shape while the actual authority decision stays outside the dispatcher.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::repair_brief::{AllowedChangeKind, SourceOfTruth};
use super::repair_plan::AcceptedRepairPlan;
use super::task_contract::RecoveryTargetHint;
use crate::safety::path_guard::resolve_user_path;
use crate::session::feedback::mask_secrets;
use crate::util::file_classify::{is_implementation_file, is_test_file};
use crate::util::workspace_paths::is_ignored_workspace_display_path;
use sha2::{Digest, Sha256};

const REPAIR_PASS_MAX_REPLACE_ALL_MATCHES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RepairIntentTextPayload<'a> {
    pub(super) old_string: &'a str,
    pub(super) new_string: &'a str,
    pub(super) reason: &'a str,
    pub(super) relative_path: &'a str,
    pub(super) current_total_edit_bytes: usize,
    pub(super) max_total_edit_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RepairIntentEdit<'a> {
    pub(super) old_string: &'a str,
    pub(super) new_string: &'a str,
    pub(super) replace_all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VerifierRepairIntent {
    pub(super) path: String,
    pub(super) old_string: String,
    pub(super) new_string: String,
    pub(super) reason: String,
    pub(super) replace_all: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VerifierRepairIntentLimits {
    pub(super) max_output_bytes: usize,
    pub(super) max_edits: usize,
    pub(super) max_reason_chars: usize,
}

/// Issue #653 (DR3-001): cheap validation can be conclusive failure or
/// unavailable. The verifier repair caller keeps this display-compatible with
/// earlier `Err(String)` paths by treating string errors as `Failed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CheapCheckOutcome {
    Failed(String),
    Unavailable,
}

impl From<String> for CheapCheckOutcome {
    fn from(message: String) -> Self {
        CheapCheckOutcome::Failed(message)
    }
}

impl std::fmt::Display for CheapCheckOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheapCheckOutcome::Failed(message) => f.write_str(message),
            CheapCheckOutcome::Unavailable => f.write_str("<cheap check unavailable>"),
        }
    }
}

#[cfg(test)]
impl CheapCheckOutcome {
    /// Test-only convenience to preserve the previous `err.contains("...")`
    /// assertion style used throughout `turn.rs::tests`. Unavailable never
    /// matches, so a test expecting a Failed message will fail loudly if the
    /// validator ever short-circuits with Unavailable instead.
    pub(super) fn contains(&self, needle: &str) -> bool {
        match self {
            CheapCheckOutcome::Failed(message) => message.contains(needle),
            CheapCheckOutcome::Unavailable => false,
        }
    }
}

/// Issue #653 (DR1-001 / DR3-002): structured weakening metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ValidationWeakening {
    pub(super) rejection: super::repair_attempt_outcome::RepairRejectionKind,
    pub(super) pattern: super::spec_authority::WeakeningPattern,
}

/// Issue #662: structured "non-unsafe" rejection signal carried inside
/// `ValidationFailure`. It maps 1:1 to repair-attempt ledger outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairRejectionSignal {
    Noop,
    Duplicate,
    Malformed,
}

/// Issue #653 (DR1-001 / DR3-001): structured verifier repair validation
/// error. `outcome` preserves the legacy cheap-check/string compatibility,
/// while `weakening` and `rejection_signal` provide typed ledger signals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ValidationFailure {
    pub(super) outcome: CheapCheckOutcome,
    pub(super) weakening: Option<ValidationWeakening>,
    pub(super) rejection_signal: Option<RepairRejectionSignal>,
}

impl ValidationFailure {
    pub(super) fn failed(message: String) -> Self {
        Self {
            outcome: CheapCheckOutcome::Failed(message),
            weakening: None,
            rejection_signal: None,
        }
    }

    pub(super) fn failed_with_signal(message: String, signal: RepairRejectionSignal) -> Self {
        Self {
            outcome: CheapCheckOutcome::Failed(message),
            weakening: None,
            rejection_signal: Some(signal),
        }
    }
}

impl From<String> for ValidationFailure {
    fn from(message: String) -> Self {
        Self::failed(message)
    }
}

impl From<CheapCheckOutcome> for ValidationFailure {
    fn from(outcome: CheapCheckOutcome) -> Self {
        Self {
            outcome,
            weakening: None,
            rejection_signal: None,
        }
    }
}

impl std::fmt::Display for ValidationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.outcome.fmt(f)
    }
}

#[cfg(test)]
impl ValidationFailure {
    /// Test-only convenience to preserve the previous `err.contains("...")`
    /// assertion style used throughout `turn.rs::tests`.
    pub(super) fn contains(&self, needle: &str) -> bool {
        self.outcome.contains(needle)
    }
}

/// Issue #653 (CB-001) / #662 (5-4-1): derive the per-attempt ledger outcome
/// from a verifier repair validation result. The caller supplies the active
/// semantic plan so non-semantic legacy validation failures stay outside the
/// lifecycle ledger.
pub(super) fn build_verifier_repair_pass_ledger_outcome(
    weakening: Option<ValidationWeakening>,
    rejection_signal: Option<RepairRejectionSignal>,
    semantic_plan: Option<&super::repair_job::SemanticRepairPlan>,
) -> Option<super::repair_attempt_outcome::RepairAttemptOutcome> {
    let plan = semantic_plan?;
    if let Some(w) = weakening {
        return Some(super::repair_attempt_outcome::RepairAttemptOutcome {
            cluster: plan.failure_cluster_id.clone(),
            role: plan.preferred_repair_role,
            kind: super::repair_attempt_outcome::RepairAttemptOutcomeKind::RejectedUnsafe {
                rejection: w.rejection,
                pattern: w.pattern,
            },
        });
    }
    let signal = rejection_signal?;
    let kind = match signal {
        RepairRejectionSignal::Noop => {
            super::repair_attempt_outcome::RepairAttemptOutcomeKind::RejectedNoop
        }
        RepairRejectionSignal::Duplicate => {
            super::repair_attempt_outcome::RepairAttemptOutcomeKind::RejectedDuplicate
        }
        RepairRejectionSignal::Malformed => {
            super::repair_attempt_outcome::RepairAttemptOutcomeKind::RejectedMalformed
        }
    };
    Some(super::repair_attempt_outcome::RepairAttemptOutcome {
        cluster: plan.failure_cluster_id.clone(),
        role: plan.preferred_repair_role,
        kind,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct RepairCandidateTestImportContractEvidence {
    pub(super) missing_modules: Vec<String>,
    pub(super) missing_imports: Vec<(String, String)>,
    pub(super) scalar_attribute_assumptions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepairCandidateApplyResult {
    pub(super) updated_contents: String,
    pub(super) used_whitespace_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ValidatedVerifierRepairEdit {
    pub(super) relative_path: String,
    pub(super) canonical_path: PathBuf,
    pub(super) updated_contents: String,
    pub(super) preimage_hash: String,
    pub(super) postimage_hash: String,
    pub(super) fingerprint: String,
}

impl ValidatedVerifierRepairEdit {
    pub(super) fn new(
        relative_path: String,
        canonical_path: PathBuf,
        original_contents: &str,
        updated_contents: String,
        fingerprint: String,
    ) -> Self {
        Self {
            relative_path,
            canonical_path,
            preimage_hash: sha256_hex(original_contents.as_bytes()),
            postimage_hash: sha256_hex(updated_contents.as_bytes()),
            updated_contents,
            fingerprint,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairIntentInputError {
    EmptyOldString,
    Noop,
    EditTooLarge,
    Markup,
    IntroducesSecret,
    SuspiciousShellPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RepairIntentPayloadValidationError {
    TargetPath(RepairIntentTargetPathError),
    Input(RepairIntentInputError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairIntentListBoundsError {
    Empty,
    TooMany,
}

impl RepairIntentListBoundsError {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::Empty => "repair intent list must not be empty",
            Self::TooMany => "repair intent list contained too many edits",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepairTargetSnapshot {
    pub(super) relative_path: String,
    pub(super) canonical_path: PathBuf,
    pub(super) contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RepairTargetReadError {
    UnsafePath,
    WorkspaceCanonicalize(String),
    Resolve(String),
    TargetCanonicalize(String),
    EscapesWorkspace,
    NotFile,
    Metadata(String),
    FileTooLarge,
    Read(String),
    NotUtf8,
}

impl RepairTargetReadError {
    pub(super) fn message(self) -> String {
        match self {
            Self::UnsafePath => "selected repair target path is not safe".to_string(),
            Self::WorkspaceCanonicalize(err) => {
                format!("failed to canonicalize workspace: {err}")
            }
            Self::Resolve(err) => err,
            Self::TargetCanonicalize(err) => {
                format!("selected repair target cannot be resolved: {err}")
            }
            Self::EscapesWorkspace => "repair intent target escapes workspace".to_string(),
            Self::NotFile => "repair intent target is not an existing file".to_string(),
            Self::Metadata(err) => format!("failed to read repair target metadata: {err}"),
            Self::FileTooLarge => "repair target file is too large".to_string(),
            Self::Read(err) => format!("failed to read repair target: {err}"),
            Self::NotUtf8 => "repair target is not valid UTF-8 text".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RepairIntentTargetPathError {
    UnsafePath,
    Resolve(String),
    TargetCanonicalize(String),
    TargetMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RepairCandidateContentError {
    CheapCheckFailed(String),
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RepairCandidateNoopError;

impl RepairCandidateNoopError {
    pub(super) fn message(self) -> &'static str {
        "repair intent applied but produced no net change to the file"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RepairCandidateDuplicateIntentError;

impl RepairCandidateDuplicateIntentError {
    pub(super) fn message(self) -> &'static str {
        "duplicate repair edit intent for the same failure"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairCandidateTestEditPlanError {
    MissingPlan,
    EmptyHypothesis,
}

impl RepairCandidateTestEditPlanError {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::MissingPlan => {
                "repair intent rejected: test edit requires SemanticRepairPlan \
                 (spec_authority + repair_hypothesis); none was constructed"
            }
            Self::EmptyHypothesis => {
                "repair intent rejected: test edit requires a non-empty \
                 repair_hypothesis in the SemanticRepairPlan"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RepairCandidateTestImportContractError {
    MissingModules(Vec<String>),
    MissingImportSymbols(Vec<(String, String)>),
    ScalarAttributeAssumptions(Vec<String>),
}

impl RepairCandidateTestImportContractError {
    pub(super) fn message(&self) -> String {
        match self {
            Self::MissingModules(modules) => format!(
                "repair intent rejected: test imports missing local module(s): {}",
                modules
                    .iter()
                    .take(4)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::MissingImportSymbols(imports) => format!(
                "repair intent rejected: test imports missing local symbol(s): {}",
                imports
                    .iter()
                    .take(4)
                    .map(|(module, name)| format!("{module}.{name}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::ScalarAttributeAssumptions(symbols) => format!(
                "repair intent rejected: test assumes attribute access on imported scalar local symbol(s): {}",
                symbols
                    .iter()
                    .take(4)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepairCandidateWeakeningError {
    pub(super) rejection: Option<super::repair_attempt_outcome::RepairRejectionKind>,
    pub(super) pattern: Option<super::spec_authority::WeakeningPattern>,
    patterns: Vec<super::spec_authority::WeakeningPattern>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepairCandidateWeakeningDetection {
    pub(super) patterns: Vec<super::spec_authority::WeakeningPattern>,
    pub(super) rejection_kind: Option<super::repair_attempt_outcome::RepairRejectionKind>,
    pub(super) target_is_test_file: bool,
}

impl RepairCandidateWeakeningError {
    pub(super) fn message(&self) -> String {
        format!(
            "repair intent rejected: test/impl weakening detected ({:?})",
            self.patterns
        )
    }
}

impl RepairIntentTargetPathError {
    pub(super) fn message(self) -> String {
        match self {
            Self::UnsafePath => {
                "repair intent path is not a safe workspace-relative path".to_string()
            }
            Self::Resolve(err) => err,
            Self::TargetCanonicalize(err) => {
                format!("repair intent target cannot be resolved: {err}")
            }
            Self::TargetMismatch => {
                "repair intent path does not match selected repair target".to_string()
            }
        }
    }
}

impl RepairIntentInputError {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::EmptyOldString => "repair intent old_string must not be empty",
            Self::Noop => "repair intent old_string and new_string are identical",
            Self::EditTooLarge => "repair intent edit is too large",
            Self::Markup => "repair intent string contained markdown or tool-call markup",
            Self::IntroducesSecret => "repair intent appears to introduce a secret",
            Self::SuspiciousShellPayload => {
                "repair intent contains suspicious shell-control payload"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairPlanTargetAuthorizationError {
    TargetMismatch,
    RoleMismatch,
    InsufficientEvidence,
    AmbiguousAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DuplicateBindingRepairError {
    still_duplicate: Vec<String>,
}

impl DuplicateBindingRepairError {
    pub(super) fn message(&self) -> String {
        format!(
            "repair intent rejected: duplicate binding still present after candidate edit ({})",
            self.still_duplicate.join(", ")
        )
    }
}

impl RepairPlanTargetAuthorizationError {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::TargetMismatch => {
                "repair intent rejected: AcceptedRepairPlan target does not match selected repair target"
            }
            Self::RoleMismatch => {
                "repair intent rejected: AcceptedRepairPlan role does not match selected repair target"
            }
            Self::InsufficientEvidence => {
                "repair intent rejected: AcceptedRepairPlan has insufficient evidence"
            }
            Self::AmbiguousAuthority => {
                "repair intent rejected: AcceptedRepairPlan authority is ambiguous"
            }
        }
    }
}

pub(super) fn validate_accepted_plan_authorizes_target(
    accepted_plan: &AcceptedRepairPlan,
    target_hint: &RecoveryTargetHint,
    relative_path: &str,
) -> Result<(), RepairPlanTargetAuthorizationError> {
    let action = &accepted_plan.action;
    if normalize_repair_path(&action.target_path) != normalize_repair_path(relative_path) {
        return Err(RepairPlanTargetAuthorizationError::TargetMismatch);
    }
    if action.target_role != target_hint.role {
        return Err(RepairPlanTargetAuthorizationError::RoleMismatch);
    }
    if action.allowed_change_kind == AllowedChangeKind::InsufficientEvidence {
        return Err(RepairPlanTargetAuthorizationError::InsufficientEvidence);
    }
    if matches!(action.source_of_truth, SourceOfTruth::Ambiguous)
        || (matches!(action.source_of_truth, SourceOfTruth::Unknown)
            && change_kind_requires_spec_authority(action.allowed_change_kind))
    {
        return Err(RepairPlanTargetAuthorizationError::AmbiguousAuthority);
    }
    Ok(())
}

pub(super) fn is_repair_path_input_safe(raw_path: &str) -> bool {
    let path = raw_path.trim();
    if path.is_empty()
        || path.contains('\0')
        || path.chars().any(|ch| ch.is_control())
        || Path::new(path).is_absolute()
        || is_ignored_workspace_display_path(path)
    {
        return false;
    }
    !Path::new(path)
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
}

pub(super) fn read_repair_target_snapshot(
    work_root: &Path,
    raw_path: &str,
    max_file_bytes: u64,
) -> Result<RepairTargetSnapshot, RepairTargetReadError> {
    if !is_repair_path_input_safe(raw_path) {
        return Err(RepairTargetReadError::UnsafePath);
    }
    let root = std::fs::canonicalize(work_root)
        .map_err(|err| RepairTargetReadError::WorkspaceCanonicalize(err.to_string()))?;
    let selected =
        resolve_user_path(work_root, raw_path).map_err(RepairTargetReadError::Resolve)?;
    let canonical = std::fs::canonicalize(&selected)
        .map_err(|err| RepairTargetReadError::TargetCanonicalize(err.to_string()))?;
    if canonical.strip_prefix(&root).is_err() {
        return Err(RepairTargetReadError::EscapesWorkspace);
    }
    if !canonical.is_file() {
        return Err(RepairTargetReadError::NotFile);
    }
    let metadata = std::fs::metadata(&canonical)
        .map_err(|err| RepairTargetReadError::Metadata(err.to_string()))?;
    if metadata.len() > max_file_bytes {
        return Err(RepairTargetReadError::FileTooLarge);
    }
    let bytes =
        std::fs::read(&canonical).map_err(|err| RepairTargetReadError::Read(err.to_string()))?;
    let contents = String::from_utf8(bytes).map_err(|_| RepairTargetReadError::NotUtf8)?;
    let relative_path = canonical
        .strip_prefix(&root)
        .map_err(|_| RepairTargetReadError::EscapesWorkspace)?
        .to_string_lossy()
        .replace('\\', "/");
    Ok(RepairTargetSnapshot {
        relative_path,
        canonical_path: canonical,
        contents,
    })
}

pub(super) fn validate_repair_intent_target_path(
    work_root: &Path,
    raw_path: &str,
    selected_canonical_path: &Path,
) -> Result<(), RepairIntentTargetPathError> {
    if !is_repair_path_input_safe(raw_path) {
        return Err(RepairIntentTargetPathError::UnsafePath);
    }
    let candidate =
        resolve_user_path(work_root, raw_path).map_err(RepairIntentTargetPathError::Resolve)?;
    let candidate = std::fs::canonicalize(&candidate)
        .map_err(|err| RepairIntentTargetPathError::TargetCanonicalize(err.to_string()))?;
    if candidate != selected_canonical_path {
        return Err(RepairIntentTargetPathError::TargetMismatch);
    }
    Ok(())
}

pub(super) fn validate_repair_candidate_contents(
    relative_path: &str,
    candidate_contents: &str,
    used_whitespace_fallback: bool,
) -> Result<(), RepairCandidateContentError> {
    use super::project_verifier::{ProjectVerifier, ProjectVerifierOutcome};

    match ProjectVerifier::for_path(relative_path) {
        None => {
            if used_whitespace_fallback && path_is_whitespace_sensitive(relative_path) {
                return Err(RepairCandidateContentError::Unavailable);
            }
            Ok(())
        }
        Some(verifier) => match verifier.check(relative_path, candidate_contents) {
            ProjectVerifierOutcome::Ok => Ok(()),
            ProjectVerifierOutcome::Failed(err) => {
                Err(RepairCandidateContentError::CheapCheckFailed(format!(
                    "repair candidate cheap check failed for {relative_path}: {err}"
                )))
            }
            ProjectVerifierOutcome::Unavailable => Err(RepairCandidateContentError::Unavailable),
        },
    }
}

pub(super) fn validate_repair_candidate_weakening_patterns(
    patterns: Vec<super::spec_authority::WeakeningPattern>,
    rejection: Option<super::repair_attempt_outcome::RepairRejectionKind>,
) -> Result<(), RepairCandidateWeakeningError> {
    if patterns.is_empty() {
        return Ok(());
    }
    Err(RepairCandidateWeakeningError {
        rejection,
        pattern: patterns.first().copied(),
        patterns,
    })
}

pub(super) fn detect_repair_candidate_weakening_patterns(
    relative_path: &str,
    original_contents: &str,
    candidate_contents: &str,
) -> RepairCandidateWeakeningDetection {
    let path = Path::new(relative_path);
    if is_test_file(path) {
        return RepairCandidateWeakeningDetection {
            patterns: super::spec_authority::detect_test_weakening(
                relative_path,
                original_contents,
                candidate_contents,
            ),
            rejection_kind: Some(super::repair_attempt_outcome::RepairRejectionKind::TestWeakening),
            target_is_test_file: true,
        };
    }
    if is_implementation_file(path) {
        return RepairCandidateWeakeningDetection {
            patterns: super::spec_authority::detect_impl_weakening(
                relative_path,
                original_contents,
                candidate_contents,
            ),
            rejection_kind: Some(super::repair_attempt_outcome::RepairRejectionKind::ImplWeakening),
            target_is_test_file: false,
        };
    }
    RepairCandidateWeakeningDetection {
        patterns: Vec::new(),
        rejection_kind: None,
        target_is_test_file: false,
    }
}

pub(super) fn apply_repair_intent_edits(
    initial_contents: &str,
    edits: &[RepairIntentEdit<'_>],
) -> Result<RepairCandidateApplyResult, String> {
    let mut contents = initial_contents.to_string();
    let mut used_whitespace_fallback = false;
    for edit in edits {
        contents = if edit.replace_all {
            apply_bounded_replace_all(&contents, edit.old_string, edit.new_string)
                .map_err(|err| format!("repair intent replace_all rejected: {err}"))?
        } else {
            let result = apply_exact_once_with_whitespace_fallback(
                &contents,
                edit.old_string,
                edit.new_string,
            )
            .map_err(|err| {
                format!(
                    "repair intent exact edit rejected: {err}; old_string_excerpt={}",
                    compact_for_repair_error(edit.old_string, 120)
                )
            })?;
            used_whitespace_fallback |= result.used_whitespace_fallback;
            result.updated_contents
        };
    }
    Ok(RepairCandidateApplyResult {
        updated_contents: contents,
        used_whitespace_fallback,
    })
}

pub(super) fn repair_intent_edits_fingerprint(
    failure_signature: &str,
    relative_path: &str,
    edits: &[RepairIntentEdit<'_>],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(failure_signature.as_bytes());
    hasher.update(b"\0");
    hasher.update(relative_path.as_bytes());
    for edit in edits {
        hasher.update(b"\0");
        hasher.update(edit.old_string.as_bytes());
        hasher.update(b"\0");
        hasher.update(edit.new_string.as_bytes());
        hasher.update(b"\0");
        hasher.update(if edit.replace_all {
            b"replace_all".as_slice()
        } else {
            b"exact_once".as_slice()
        });
    }
    format!("{:x}", hasher.finalize())
}

pub(super) fn validate_repair_candidate_changed(
    original_contents: &str,
    updated_contents: &str,
) -> Result<(), RepairCandidateNoopError> {
    if original_contents == updated_contents {
        return Err(RepairCandidateNoopError);
    }
    Ok(())
}

pub(super) fn validate_repair_intent_not_replayed(
    applied_repair_intents: &[String],
    fingerprint: &str,
) -> Result<(), RepairCandidateDuplicateIntentError> {
    if applied_repair_intents
        .iter()
        .any(|applied| applied == fingerprint)
    {
        return Err(RepairCandidateDuplicateIntentError);
    }
    Ok(())
}

pub(super) fn validate_repair_intent_list_bounds(
    intent_count: usize,
    max_intents: usize,
) -> Result<(), RepairIntentListBoundsError> {
    if intent_count == 0 {
        return Err(RepairIntentListBoundsError::Empty);
    }
    if intent_count > max_intents {
        return Err(RepairIntentListBoundsError::TooMany);
    }
    Ok(())
}

pub(super) fn validate_repair_intents_and_build_edit_payloads<'a>(
    work_root: &Path,
    selected_canonical_path: &Path,
    relative_path: &str,
    intents: &'a [VerifierRepairIntent],
    max_total_edit_bytes: usize,
) -> Result<Vec<RepairIntentEdit<'a>>, RepairIntentPayloadValidationError> {
    let mut total_edit_bytes = 0usize;
    let mut edit_payloads = Vec::with_capacity(intents.len());
    for intent in intents {
        validate_repair_intent_target_path(work_root, &intent.path, selected_canonical_path)
            .map_err(RepairIntentPayloadValidationError::TargetPath)?;
        total_edit_bytes = validate_repair_intent_text_payload(RepairIntentTextPayload {
            old_string: &intent.old_string,
            new_string: &intent.new_string,
            reason: &intent.reason,
            relative_path,
            current_total_edit_bytes: total_edit_bytes,
            max_total_edit_bytes,
        })
        .map_err(RepairIntentPayloadValidationError::Input)?;
        edit_payloads.push(RepairIntentEdit {
            old_string: &intent.old_string,
            new_string: &intent.new_string,
            replace_all: intent.replace_all,
        });
    }
    Ok(edit_payloads)
}

pub(super) fn validate_test_edit_semantic_plan(
    is_test_file: bool,
    accepted_plan_present: bool,
    repair_hypothesis: Option<&str>,
) -> Result<(), RepairCandidateTestEditPlanError> {
    if !is_test_file || accepted_plan_present {
        return Ok(());
    }
    let repair_hypothesis =
        repair_hypothesis.ok_or(RepairCandidateTestEditPlanError::MissingPlan)?;
    if repair_hypothesis.trim().is_empty() {
        return Err(RepairCandidateTestEditPlanError::EmptyHypothesis);
    }
    Ok(())
}

pub(super) fn validate_test_import_contract_evidence(
    evidence: RepairCandidateTestImportContractEvidence,
) -> Result<(), RepairCandidateTestImportContractError> {
    if !evidence.missing_modules.is_empty() {
        return Err(RepairCandidateTestImportContractError::MissingModules(
            evidence.missing_modules,
        ));
    }
    if !evidence.missing_imports.is_empty() {
        return Err(
            RepairCandidateTestImportContractError::MissingImportSymbols(evidence.missing_imports),
        );
    }
    if !evidence.scalar_attribute_assumptions.is_empty() {
        return Err(
            RepairCandidateTestImportContractError::ScalarAttributeAssumptions(
                evidence.scalar_attribute_assumptions,
            ),
        );
    }
    Ok(())
}

pub(super) fn parse_verifier_repair_patch_proposal_reply(
    reply: &str,
    limits: VerifierRepairIntentLimits,
) -> Result<super::patch_proposal::PatchProposal, String> {
    if reply.len() > limits.max_output_bytes {
        return Err("repair reply exceeded output cap".to_string());
    }
    super::patch_proposal::parse_patch_proposal_reply(reply)
        .map_err(patch_proposal_error_to_repair_intent_error)
}

pub(super) fn patch_proposal_to_verifier_repair_intents(
    proposal: super::patch_proposal::PatchProposal,
    limits: VerifierRepairIntentLimits,
) -> Result<Vec<VerifierRepairIntent>, String> {
    if proposal.edits.is_empty() {
        return Err("repair reply edits array must not be empty".to_string());
    }
    if proposal.edits.len() > limits.max_edits {
        return Err("repair reply contained too many edits".to_string());
    }
    let root_reason = if proposal.explanation.is_empty() {
        proposal.risk.as_str()
    } else {
        proposal.explanation.as_str()
    };
    Ok(proposal
        .edits
        .into_iter()
        .map(|edit| VerifierRepairIntent {
            path: proposal.target_path.clone(),
            old_string: edit.old_string,
            new_string: edit.new_string,
            reason: if edit.reason.is_empty() {
                compact_repair_intent_reason(root_reason, limits.max_reason_chars)
            } else {
                compact_repair_intent_reason(&edit.reason, limits.max_reason_chars)
            },
            replace_all: edit.replace_all,
        })
        .collect())
}

fn patch_proposal_error_to_repair_intent_error(
    err: super::patch_proposal::PatchProposalError,
) -> String {
    match err {
        super::patch_proposal::PatchProposalError::ToolMarkup => {
            "repair reply contained tool-call shaped markup".to_string()
        }
        super::patch_proposal::PatchProposalError::JsonMissing => {
            "repair reply must contain a JSON object".to_string()
        }
        super::patch_proposal::PatchProposalError::JsonMalformed => {
            "repair reply was not valid JSON".to_string()
        }
        super::patch_proposal::PatchProposalError::ObjectMissing => {
            "repair reply must be a JSON object".to_string()
        }
        super::patch_proposal::PatchProposalError::MissingField(field) => {
            if field == "target_path" {
                "repair reply missing string field: path".to_string()
            } else {
                format!("repair reply missing string field: {field}")
            }
        }
        super::patch_proposal::PatchProposalError::EditsEmpty => {
            "repair reply edits array must not be empty".to_string()
        }
        super::patch_proposal::PatchProposalError::TooManyEdits => {
            "repair reply contained too many edits".to_string()
        }
        super::patch_proposal::PatchProposalError::EditMalformed => {
            "repair reply edits must be JSON objects".to_string()
        }
    }
}

fn compact_repair_intent_reason(input: &str, max_chars: usize) -> String {
    let masked = mask_secrets(input);
    let collapsed = masked.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars_with_ellipsis(&collapsed, max_chars)
}

fn truncate_chars_with_ellipsis(input: &str, max_chars: usize) -> String {
    match input.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => {
            let mut out = String::with_capacity(byte_idx + 3);
            out.push_str(&input[..byte_idx]);
            out.push_str("...");
            out
        }
        None => input.to_string(),
    }
}

pub(super) fn validate_repair_intent_text_payload(
    input: RepairIntentTextPayload<'_>,
) -> Result<usize, RepairIntentInputError> {
    if input.old_string.is_empty() {
        return Err(RepairIntentInputError::EmptyOldString);
    }
    if input.old_string == input.new_string {
        return Err(RepairIntentInputError::Noop);
    }
    let total_edit_bytes = input
        .current_total_edit_bytes
        .saturating_add(input.old_string.len())
        .saturating_add(input.new_string.len());
    if total_edit_bytes > input.max_total_edit_bytes {
        return Err(RepairIntentInputError::EditTooLarge);
    }
    if contains_tool_markup(input.old_string)
        || contains_tool_markup(input.new_string)
        || contains_tool_or_markdown(input.reason)
    {
        return Err(RepairIntentInputError::Markup);
    }
    if introduces_obvious_secret(input.old_string, input.new_string) {
        return Err(RepairIntentInputError::IntroducesSecret);
    }
    if !path_allows_shell_controls(input.relative_path)
        && contains_suspicious_shell_payload(input.new_string)
    {
        return Err(RepairIntentInputError::SuspiciousShellPayload);
    }
    Ok(total_edit_bytes)
}

pub(super) fn validate_duplicate_binding_repair_candidate(
    relative_path: &str,
    context: &super::repair_job::RepairJob,
    before: &str,
    after: &str,
) -> Result<(), DuplicateBindingRepairError> {
    let duplicate_names = duplicate_binding_names_from_verifier_context(context);
    if duplicate_names.is_empty() {
        return Ok(());
    }
    let before_counts = source_binding_counts_for_duplicate_guard(relative_path, before);
    let after_counts = source_binding_counts_for_duplicate_guard(relative_path, after);
    let mut still_duplicate = Vec::new();
    for name in duplicate_names {
        let before_count = before_counts.get(&name).copied().unwrap_or(0);
        let after_count = after_counts.get(&name).copied().unwrap_or(0);
        if after_count > 1 && after_count >= before_count {
            still_duplicate.push(format!("{name}={after_count}"));
        }
    }
    if still_duplicate.is_empty() {
        return Ok(());
    }
    Err(DuplicateBindingRepairError { still_duplicate })
}

fn change_kind_requires_spec_authority(kind: AllowedChangeKind) -> bool {
    matches!(
        kind,
        AllowedChangeKind::FixImplementationBehavior
            | AllowedChangeKind::FixGeneratedTestExpectation
    )
}

fn normalize_repair_path(path: &str) -> String {
    path.trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

fn contains_tool_or_markdown(value: &str) -> bool {
    value.contains("```") || contains_tool_markup(value)
}

fn contains_tool_markup(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("<anvil_tool_call") || lower.contains("</anvil_tool_call>")
}

fn introduces_obvious_secret(old: &str, new: &str) -> bool {
    let old_masked = crate::session::feedback::mask_secrets(old);
    let new_masked = crate::session::feedback::mask_secrets(new);
    old_masked == old && new_masked != new
}

fn path_allows_shell_controls(relative_path: &str) -> bool {
    let path = Path::new(relative_path);
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        filename.as_str(),
        "makefile" | "justfile" | "taskfile.yml" | "taskfile.yaml"
    ) {
        return true;
    }
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "sh" | "bash" | "zsh" | "fish" | "ps1" | "cmd" | "bat"
    )
}

fn contains_suspicious_shell_payload(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "rm -rf",
        "curl ",
        "wget ",
        "| sh",
        "| bash",
        "bash -c",
        "sh -c",
        "powershell",
        "chmod +x",
        "mkfs",
        "dd if=",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

fn path_is_whitespace_sensitive(relative_path: &str) -> bool {
    matches!(
        Path::new(relative_path)
            .extension()
            .and_then(|extension| extension.to_str()),
        Some("py") | Some("pyw") | Some("yaml") | Some("yml")
    )
}

fn apply_exact_once_with_whitespace_fallback(
    contents: &str,
    old: &str,
    new: &str,
) -> Result<RepairCandidateApplyResult, String> {
    match crate::tools::edit::apply_exact_once(contents, old, new) {
        Ok(updated) => Ok(RepairCandidateApplyResult {
            updated_contents: updated,
            used_whitespace_fallback: false,
        }),
        Err(err) if err == "old_string was not found" => {
            apply_unique_whitespace_normalized_replacement(contents, old, new)
                .map(|updated| RepairCandidateApplyResult {
                    updated_contents: updated,
                    used_whitespace_fallback: true,
                })
                .map_err(|fallback_err| {
                    format!("{err}; whitespace fallback rejected: {fallback_err}")
                })
        }
        Err(err) => Err(err),
    }
}

fn apply_unique_whitespace_normalized_replacement(
    contents: &str,
    old: &str,
    new: &str,
) -> Result<String, String> {
    if old.trim().len() < 16 {
        return Err("old_string is too short for whitespace-normalized matching".to_string());
    }
    let tokens = old.split_whitespace().collect::<Vec<_>>();
    if tokens.len() < 2 {
        return Err("old_string has too few non-whitespace tokens".to_string());
    }

    let include_leading_whitespace = old.chars().next().is_some_and(|ch| ch.is_whitespace());
    let include_trailing_whitespace = old.chars().next_back().is_some_and(|ch| ch.is_whitespace());
    let mut matches = Vec::new();
    let first = tokens[0];
    let mut search_from = 0usize;

    while search_from <= contents.len() {
        let Some(relative_start) = contents[search_from..].find(first) else {
            break;
        };
        let token_start = search_from + relative_start;
        let token_end = token_start + first.len();
        search_from = token_end;

        if !is_whitespace_boundary_before(contents, token_start) {
            continue;
        }

        let mut pos = token_end;
        let mut matched = true;
        for token in tokens.iter().skip(1) {
            let before_skip = pos;
            pos = skip_whitespace(contents, pos);
            if pos == before_skip || !contents[pos..].starts_with(token) {
                matched = false;
                break;
            }
            pos += token.len();
        }
        if !matched {
            continue;
        }

        let span_end = if include_trailing_whitespace {
            let extended = skip_whitespace(contents, pos);
            if extended == pos {
                continue;
            }
            extended
        } else if is_whitespace_boundary_after(contents, pos) {
            pos
        } else {
            continue;
        };
        let span_start = if include_leading_whitespace {
            let extended = backtrack_whitespace(contents, token_start);
            if extended == token_start {
                continue;
            }
            extended
        } else {
            token_start
        };

        matches.push((span_start, span_end));
        if matches.len() > 1 {
            return Err(
                "old_string matched more than once after whitespace normalization".to_string(),
            );
        }
    }

    let Some((start, end)) = matches.into_iter().next() else {
        return Err("old_string was not found after whitespace normalization".to_string());
    };
    let mut updated = String::with_capacity(contents.len() + new.len().saturating_sub(end - start));
    updated.push_str(&contents[..start]);
    updated.push_str(new);
    updated.push_str(&contents[end..]);
    Ok(updated)
}

fn skip_whitespace(value: &str, mut pos: usize) -> usize {
    while pos < value.len() {
        let Some(ch) = value[pos..].chars().next() else {
            break;
        };
        if !ch.is_whitespace() {
            break;
        }
        pos += ch.len_utf8();
    }
    pos
}

fn backtrack_whitespace(value: &str, mut pos: usize) -> usize {
    while pos > 0 {
        let Some((previous_pos, ch)) = value[..pos].char_indices().next_back() else {
            break;
        };
        if !ch.is_whitespace() {
            break;
        }
        pos = previous_pos;
    }
    pos
}

fn is_whitespace_boundary_before(value: &str, pos: usize) -> bool {
    pos == 0
        || value[..pos]
            .chars()
            .next_back()
            .is_some_and(|ch| ch.is_whitespace())
}

fn is_whitespace_boundary_after(value: &str, pos: usize) -> bool {
    pos == value.len()
        || value[pos..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_whitespace())
}

fn apply_bounded_replace_all(contents: &str, old: &str, new: &str) -> Result<String, String> {
    if old.trim().is_empty() || old.len() < 2 {
        return Err("replace_all old_string is too broad".to_string());
    }
    let match_count = contents.matches(old).count();
    if match_count == 0 {
        return Err("old_string was not found".to_string());
    }
    if match_count > REPAIR_PASS_MAX_REPLACE_ALL_MATCHES {
        return Err("old_string matched too many locations".to_string());
    }
    Ok(contents.replace(old, new))
}

fn compact_for_repair_error(input: &str, max_chars: usize) -> String {
    let masked = crate::session::feedback::mask_secrets(input);
    let collapsed = masked.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&collapsed, max_chars)
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    input.chars().take(max_chars).collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn duplicate_binding_names_from_verifier_context(
    context: &super::repair_job::RepairJob,
) -> HashSet<String> {
    let diagnostic_text = format!(
        "{}\n{}\n{}",
        context.failure_signature,
        context.output_excerpt,
        context.repair_error.as_deref().unwrap_or("")
    );
    if !text_mentions_duplicate_binding_failure(&diagnostic_text) {
        return HashSet::new();
    }
    quoted_safe_identifiers(&diagnostic_text)
}

fn text_mentions_duplicate_binding_failure(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("defined multiple times")
        || lower.contains("redefined")
        || lower.contains("duplicate definition")
        || lower.contains("already been declared")
        || lower.contains("already defined")
}

fn quoted_safe_identifiers(text: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    for quote in ['`', '\'', '"'] {
        let mut rest = text;
        while let Some(start) = rest.find(quote) {
            let after_start = &rest[start + quote.len_utf8()..];
            let Some(end) = after_start.find(quote) else {
                break;
            };
            let candidate = &after_start[..end];
            if source_identifier_is_safe(candidate) {
                names.insert(candidate.to_string());
            }
            rest = &after_start[end + quote.len_utf8()..];
        }
    }
    names
}

fn source_binding_counts_for_duplicate_guard(
    relative_path: &str,
    contents: &str,
) -> HashMap<String, usize> {
    let extension = Path::new(relative_path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut counts = HashMap::new();
    for line in contents.lines() {
        let trimmed = line.trim_start();
        let binding = match extension.as_str() {
            "rs" => rust_binding_name(trimmed),
            "py" | "pyw" => python_binding_name(trimmed),
            "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => js_like_binding_name(trimmed),
            _ => None,
        };
        if let Some(name) = binding {
            *counts.entry(name).or_insert(0) += 1;
        }
    }
    counts
}

fn rust_binding_name(line: &str) -> Option<String> {
    if line.starts_with("//") || line.starts_with("/*") || line.starts_with('*') {
        return None;
    }
    let mut tokens = line.split_whitespace().peekable();
    while let Some(token) = tokens.peek().copied() {
        if token == "pub"
            || token.starts_with("pub(")
            || matches!(token, "async" | "unsafe" | "extern")
        {
            tokens.next();
        } else {
            break;
        }
    }
    let keyword = tokens.next()?;
    if keyword == "const" && tokens.peek().copied() == Some("fn") {
        tokens.next();
        return token_to_source_identifier(tokens.next()?);
    }
    if !matches!(
        keyword,
        "fn" | "struct" | "enum" | "trait" | "type" | "const" | "static" | "mod"
    ) {
        return None;
    }
    token_to_source_identifier(tokens.next()?)
}

fn python_binding_name(line: &str) -> Option<String> {
    if line.starts_with('#') || line.starts_with('@') {
        return None;
    }
    let rest = line
        .strip_prefix("async def ")
        .or_else(|| line.strip_prefix("def "));
    if let Some(rest) = rest {
        return token_to_source_identifier(rest);
    }
    line.strip_prefix("class ")
        .and_then(token_to_source_identifier)
}

fn js_like_binding_name(line: &str) -> Option<String> {
    if line.starts_with("//") || line.starts_with("/*") || line.starts_with('*') {
        return None;
    }
    let mut tokens = line.split_whitespace().peekable();
    while let Some(token) = tokens.peek().copied() {
        if matches!(token, "export" | "default" | "async" | "declare") {
            tokens.next();
        } else {
            break;
        }
    }
    let keyword = tokens.next()?;
    match keyword {
        "function" | "class" => token_to_source_identifier(tokens.next()?),
        "const" | "let" | "var" => token_to_source_identifier(tokens.next()?),
        _ => None,
    }
}

fn token_to_source_identifier(token: &str) -> Option<String> {
    let ident = token
        .trim_start_matches("r#")
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '$')
        .collect::<String>();
    source_identifier_is_safe(&ident).then_some(ident)
}

fn source_identifier_is_safe(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_' || first == '$')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
}

#[cfg(test)]
mod tests {
    use super::super::repair_action::RepairAction;
    use super::super::repair_brief::RepairBriefSource;
    use super::super::task_contract::ArtifactRole;
    use super::*;
    use tempfile::tempdir;

    fn accepted_plan(
        role: ArtifactRole,
        path: &str,
        allowed_change_kind: AllowedChangeKind,
        source_of_truth: SourceOfTruth,
    ) -> AcceptedRepairPlan {
        AcceptedRepairPlan {
            action: RepairAction {
                target_role: role,
                target_path: path.to_string(),
                allowed_change_kind,
                source_of_truth,
                budget: 2,
                brief_confidence: 0.9,
            },
            proposal_source: RepairBriefSource::DiagnosticLlm,
        }
    }

    fn target(role: ArtifactRole, path: &str) -> RecoveryTargetHint {
        RecoveryTargetHint {
            role,
            path: path.to_string(),
            reason: "test target".to_string(),
        }
    }

    #[test]
    fn accepted_plan_allows_structural_test_setup_fix_with_unknown_authority() {
        let plan = accepted_plan(
            ArtifactRole::Test,
            "./tests/lib.rs",
            AllowedChangeKind::FixTestImportOrSetup,
            SourceOfTruth::Unknown,
        );

        assert!(
            validate_accepted_plan_authorizes_target(
                &plan,
                &target(ArtifactRole::Test, "tests/lib.rs"),
                "tests/lib.rs",
            )
            .is_ok()
        );
    }

    #[test]
    fn accepted_plan_rejects_unknown_authority_for_behavior_fix() {
        let plan = accepted_plan(
            ArtifactRole::Implementation,
            "src/lib.rs",
            AllowedChangeKind::FixImplementationBehavior,
            SourceOfTruth::Unknown,
        );

        assert_eq!(
            validate_accepted_plan_authorizes_target(
                &plan,
                &target(ArtifactRole::Implementation, "src/lib.rs"),
                "src/lib.rs",
            )
            .unwrap_err(),
            RepairPlanTargetAuthorizationError::AmbiguousAuthority
        );
    }

    #[test]
    fn accepted_plan_rejects_role_mismatch() {
        let plan = accepted_plan(
            ArtifactRole::Implementation,
            "src/lib.rs",
            AllowedChangeKind::FixImplementationBehavior,
            SourceOfTruth::BehaviorContract,
        );

        assert_eq!(
            validate_accepted_plan_authorizes_target(
                &plan,
                &target(ArtifactRole::Test, "src/lib.rs"),
                "src/lib.rs",
            )
            .unwrap_err(),
            RepairPlanTargetAuthorizationError::RoleMismatch
        );
    }

    #[test]
    fn repair_path_input_rejects_escape_and_ignored_state() {
        assert!(!is_repair_path_input_safe("../src/lib.rs"));
        assert!(!is_repair_path_input_safe("/tmp/src/lib.rs"));
        assert!(!is_repair_path_input_safe(".anvil-state/session.json"));
        assert!(is_repair_path_input_safe("src/lib.rs"));
    }

    #[test]
    fn repair_target_snapshot_reads_safe_workspace_file() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn answer() -> i32 { 42 }\n",
        )
        .unwrap();

        let snapshot = read_repair_target_snapshot(dir.path(), "src/lib.rs", 1024).unwrap();

        assert_eq!(snapshot.relative_path, "src/lib.rs");
        assert_eq!(snapshot.contents, "pub fn answer() -> i32 { 42 }\n");
        assert_eq!(
            snapshot.canonical_path,
            std::fs::canonicalize(dir.path().join("src/lib.rs")).unwrap()
        );
    }

    #[test]
    fn repair_target_snapshot_rejects_unsafe_or_large_file() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "large").unwrap();

        assert_eq!(
            read_repair_target_snapshot(dir.path(), "../src/lib.rs", 1024).unwrap_err(),
            RepairTargetReadError::UnsafePath
        );
        assert_eq!(
            read_repair_target_snapshot(dir.path(), "src/lib.rs", 2).unwrap_err(),
            RepairTargetReadError::FileTooLarge
        );
    }

    #[test]
    fn repair_intent_target_path_must_match_selected_target() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "pub fn a() {}\n").unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let selected = std::fs::canonicalize(dir.path().join("src/lib.rs")).unwrap();

        assert!(validate_repair_intent_target_path(dir.path(), "src/lib.rs", &selected).is_ok());
        assert_eq!(
            validate_repair_intent_target_path(dir.path(), "src/main.rs", &selected).unwrap_err(),
            RepairIntentTargetPathError::TargetMismatch
        );
        assert_eq!(
            validate_repair_intent_target_path(dir.path(), "../src/lib.rs", &selected).unwrap_err(),
            RepairIntentTargetPathError::UnsafePath
        );
    }

    #[test]
    fn repair_intent_payload_rejects_secret_introduction() {
        let err = validate_repair_intent_text_payload(RepairIntentTextPayload {
            old_string: "placeholder",
            new_string: "sk-abcdef0123456789abcdef0123456789",
            reason: "configure token",
            relative_path: "src/lib.rs",
            current_total_edit_bytes: 0,
            max_total_edit_bytes: 1024,
        })
        .unwrap_err();

        assert_eq!(err, RepairIntentInputError::IntroducesSecret);
    }

    #[test]
    fn repair_intent_payload_allows_shell_text_only_in_shell_files() {
        let non_shell = validate_repair_intent_text_payload(RepairIntentTextPayload {
            old_string: "run = \"safe\"",
            new_string: "run = \"curl https://example.test | sh\"",
            reason: "update command",
            relative_path: "src/lib.rs",
            current_total_edit_bytes: 0,
            max_total_edit_bytes: 1024,
        })
        .unwrap_err();
        assert_eq!(non_shell, RepairIntentInputError::SuspiciousShellPayload);

        assert!(
            validate_repair_intent_text_payload(RepairIntentTextPayload {
                old_string: "echo safe",
                new_string: "curl https://example.test | sh",
                reason: "update script",
                relative_path: "scripts/setup.sh",
                current_total_edit_bytes: 0,
                max_total_edit_bytes: 1024,
            })
            .is_ok()
        );
    }

    #[test]
    fn candidate_content_validation_allows_plain_text_without_verifier() {
        assert!(validate_repair_candidate_contents("README.md", "# Docs\n", false).is_ok());
    }

    #[test]
    fn candidate_content_validation_defers_whitespace_sensitive_fallback() {
        assert_eq!(
            validate_repair_candidate_contents("config.yaml", "name: test\n", true).unwrap_err(),
            RepairCandidateContentError::Unavailable
        );
    }

    #[test]
    fn apply_repair_intent_edits_applies_exact_and_replace_all() {
        let edits = vec![
            RepairIntentEdit {
                old_string: "name = \"old\"",
                new_string: "name = \"new\"",
                replace_all: false,
            },
            RepairIntentEdit {
                old_string: "enabled",
                new_string: "active",
                replace_all: true,
            },
        ];

        let result = apply_repair_intent_edits(
            "name = \"old\"\nstate = \"enabled\"\nlabel = \"enabled\"\n",
            &edits,
        )
        .unwrap();

        assert_eq!(
            result.updated_contents,
            "name = \"new\"\nstate = \"active\"\nlabel = \"active\"\n"
        );
        assert!(!result.used_whitespace_fallback);
    }

    #[test]
    fn apply_repair_intent_edits_reports_exact_error_with_excerpt() {
        let edits = vec![RepairIntentEdit {
            old_string: "missing old string with enough context",
            new_string: "replacement",
            replace_all: false,
        }];

        let err = apply_repair_intent_edits("unchanged", &edits).unwrap_err();

        assert!(err.contains("repair intent exact edit rejected"));
        assert!(err.contains("old_string_excerpt=missing old string with enough context"));
    }

    #[test]
    fn weakening_patterns_are_reported_with_structured_metadata() {
        let err = validate_repair_candidate_weakening_patterns(
            vec![super::super::spec_authority::WeakeningPattern::AssertionDeleted],
            Some(super::super::repair_attempt_outcome::RepairRejectionKind::TestWeakening),
        )
        .unwrap_err();

        assert_eq!(
            err.rejection,
            Some(super::super::repair_attempt_outcome::RepairRejectionKind::TestWeakening)
        );
        assert_eq!(
            err.pattern,
            Some(super::super::spec_authority::WeakeningPattern::AssertionDeleted)
        );
        assert_eq!(
            err.message(),
            "repair intent rejected: test/impl weakening detected ([AssertionDeleted])"
        );
    }

    #[test]
    fn weakening_detection_dispatches_test_paths() {
        let detection = detect_repair_candidate_weakening_patterns(
            "tests/test_main.py",
            "def test_x():\n    assert foo() == 1\n",
            "def test_x():\n    assert True\n",
        );

        assert!(detection.target_is_test_file);
        assert_eq!(
            detection.rejection_kind,
            Some(super::super::repair_attempt_outcome::RepairRejectionKind::TestWeakening)
        );
        assert!(
            detection
                .patterns
                .contains(&super::super::spec_authority::WeakeningPattern::AssertTrueWeakening)
        );
    }

    #[test]
    fn weakening_detection_ignores_non_code_paths() {
        let detection = detect_repair_candidate_weakening_patterns("README.md", "before", "after");

        assert!(!detection.target_is_test_file);
        assert_eq!(detection.rejection_kind, None);
        assert!(detection.patterns.is_empty());
    }

    #[test]
    fn validated_repair_edit_constructor_hashes_pre_and_post_images() {
        let edit = ValidatedVerifierRepairEdit::new(
            "src/lib.rs".to_string(),
            PathBuf::from("/tmp/src/lib.rs"),
            "before",
            "after".to_string(),
            "fp".to_string(),
        );

        assert_eq!(edit.relative_path, "src/lib.rs");
        assert_eq!(edit.updated_contents, "after");
        assert_eq!(edit.fingerprint, "fp");
        assert_ne!(edit.preimage_hash, edit.postimage_hash);
        assert_eq!(edit.preimage_hash.len(), 64);
        assert_eq!(edit.postimage_hash.len(), 64);
    }

    #[test]
    fn repair_intent_fingerprint_is_stable_and_distinguishes_replace_mode() {
        let exact = vec![RepairIntentEdit {
            old_string: "a",
            new_string: "b",
            replace_all: false,
        }];
        let replace_all = vec![RepairIntentEdit {
            old_string: "a",
            new_string: "b",
            replace_all: true,
        }];

        let first = repair_intent_edits_fingerprint("failure", "src/lib.rs", &exact);
        let second = repair_intent_edits_fingerprint("failure", "src/lib.rs", &exact);
        let different_mode = repair_intent_edits_fingerprint("failure", "src/lib.rs", &replace_all);

        assert_eq!(first, second);
        assert_ne!(first, different_mode);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn candidate_changed_validation_rejects_noop_candidate() {
        let err = validate_repair_candidate_changed("same", "same").unwrap_err();
        assert_eq!(err, RepairCandidateNoopError);
        assert_eq!(
            err.message(),
            "repair intent applied but produced no net change to the file"
        );
        assert!(validate_repair_candidate_changed("before", "after").is_ok());
    }

    #[test]
    fn duplicate_intent_validation_rejects_replayed_fingerprint() {
        let applied = vec!["abc123".to_string()];

        let err = validate_repair_intent_not_replayed(&applied, "abc123").unwrap_err();
        assert_eq!(err, RepairCandidateDuplicateIntentError);
        assert_eq!(
            err.message(),
            "duplicate repair edit intent for the same failure"
        );
        assert!(validate_repair_intent_not_replayed(&applied, "other").is_ok());
    }

    #[test]
    fn patch_proposal_conversion_builds_bounded_repair_intents() {
        let proposal = super::super::patch_proposal::PatchProposal {
            target_path: "app/main.py".to_string(),
            edits: vec![super::super::patch_proposal::PatchEdit {
                old_string: "return 201".to_string(),
                new_string: "return 200".to_string(),
                reason: "  align   status   code  ".to_string(),
                replace_all: false,
            }],
            explanation: "fallback explanation".to_string(),
            risk: "low".to_string(),
        };
        let limits = VerifierRepairIntentLimits {
            max_output_bytes: 128,
            max_edits: 4,
            max_reason_chars: 20,
        };

        let intents = patch_proposal_to_verifier_repair_intents(proposal, limits).unwrap();

        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].path, "app/main.py");
        assert_eq!(intents[0].old_string, "return 201");
        assert_eq!(intents[0].new_string, "return 200");
        assert_eq!(intents[0].reason, "align status code");
        assert!(!intents[0].replace_all);
    }

    #[test]
    fn patch_proposal_reply_parser_rejects_output_over_cap() {
        let limits = VerifierRepairIntentLimits {
            max_output_bytes: 4,
            max_edits: 4,
            max_reason_chars: 20,
        };

        let err = parse_verifier_repair_patch_proposal_reply("{\"target_path\":\"x\"}", limits)
            .unwrap_err();

        assert_eq!(err, "repair reply exceeded output cap");
    }

    #[test]
    fn repair_intent_payload_validation_builds_normalized_edit_payloads() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let selected = dir.path().join("src/lib.rs");
        std::fs::write(&selected, "fn value() -> i32 { 1 }\n").unwrap();
        let selected = std::fs::canonicalize(selected).unwrap();
        let intents = vec![VerifierRepairIntent {
            path: "src/lib.rs".to_string(),
            old_string: "1".to_string(),
            new_string: "2".to_string(),
            reason: "fix value".to_string(),
            replace_all: false,
        }];

        let payloads = validate_repair_intents_and_build_edit_payloads(
            dir.path(),
            &selected,
            "src/lib.rs",
            &intents,
            128,
        )
        .unwrap();

        assert_eq!(
            payloads,
            vec![RepairIntentEdit {
                old_string: "1",
                new_string: "2",
                replace_all: false,
            }]
        );
    }

    #[test]
    fn repair_intent_payload_validation_preserves_input_error_kind() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let selected = dir.path().join("src/lib.rs");
        std::fs::write(&selected, "fn value() -> i32 { 1 }\n").unwrap();
        let selected = std::fs::canonicalize(selected).unwrap();
        let intents = vec![VerifierRepairIntent {
            path: "src/lib.rs".to_string(),
            old_string: "same".to_string(),
            new_string: "same".to_string(),
            reason: "noop".to_string(),
            replace_all: false,
        }];

        let err = validate_repair_intents_and_build_edit_payloads(
            dir.path(),
            &selected,
            "src/lib.rs",
            &intents,
            128,
        )
        .unwrap_err();

        assert_eq!(
            err,
            RepairIntentPayloadValidationError::Input(RepairIntentInputError::Noop)
        );
    }

    #[test]
    fn repair_intent_list_bounds_rejects_empty_and_too_many() {
        assert_eq!(
            validate_repair_intent_list_bounds(0, 2).unwrap_err(),
            RepairIntentListBoundsError::Empty
        );
        assert_eq!(
            validate_repair_intent_list_bounds(3, 2).unwrap_err(),
            RepairIntentListBoundsError::TooMany
        );
        assert!(validate_repair_intent_list_bounds(2, 2).is_ok());
    }

    #[test]
    fn test_edit_semantic_plan_validation_requires_plan_and_hypothesis() {
        let missing = validate_test_edit_semantic_plan(true, false, None).unwrap_err();
        assert_eq!(missing, RepairCandidateTestEditPlanError::MissingPlan);
        assert_eq!(
            missing.message(),
            "repair intent rejected: test edit requires SemanticRepairPlan \
                 (spec_authority + repair_hypothesis); none was constructed"
        );

        let empty = validate_test_edit_semantic_plan(true, false, Some("   ")).unwrap_err();
        assert_eq!(empty, RepairCandidateTestEditPlanError::EmptyHypothesis);
        assert_eq!(
            empty.message(),
            "repair intent rejected: test edit requires a non-empty \
                 repair_hypothesis in the SemanticRepairPlan"
        );

        assert!(validate_test_edit_semantic_plan(true, false, Some("fix expectation")).is_ok());
        assert!(validate_test_edit_semantic_plan(false, false, None).is_ok());
        assert!(validate_test_edit_semantic_plan(true, true, None).is_ok());
    }

    #[test]
    fn test_import_contract_evidence_rejects_missing_modules_first() {
        let err =
            validate_test_import_contract_evidence(RepairCandidateTestImportContractEvidence {
                missing_modules: vec!["missing_pkg".to_string()],
                missing_imports: vec![("app.main".to_string(), "app".to_string())],
                scalar_attribute_assumptions: vec!["app.value".to_string()],
            })
            .unwrap_err();

        assert_eq!(
            err.message(),
            "repair intent rejected: test imports missing local module(s): missing_pkg"
        );
    }

    #[test]
    fn test_import_contract_evidence_rejects_missing_symbols() {
        let err =
            validate_test_import_contract_evidence(RepairCandidateTestImportContractEvidence {
                missing_imports: vec![("app.main".to_string(), "app".to_string())],
                ..RepairCandidateTestImportContractEvidence::default()
            })
            .unwrap_err();

        assert_eq!(
            err.message(),
            "repair intent rejected: test imports missing local symbol(s): app.main.app"
        );
    }

    #[test]
    fn test_import_contract_evidence_rejects_scalar_attribute_assumptions() {
        let err =
            validate_test_import_contract_evidence(RepairCandidateTestImportContractEvidence {
                scalar_attribute_assumptions: vec!["app.COUNTER.value".to_string()],
                ..RepairCandidateTestImportContractEvidence::default()
            })
            .unwrap_err();

        assert_eq!(
            err.message(),
            "repair intent rejected: test assumes attribute access on imported scalar local symbol(s): app.COUNTER.value"
        );
        assert!(
            validate_test_import_contract_evidence(
                RepairCandidateTestImportContractEvidence::default()
            )
            .is_ok()
        );
    }

    #[test]
    fn duplicate_binding_candidate_rejects_unresolved_duplicate() {
        let context = super::super::repair_job::RepairJob {
            failure_signature: "function `build` is already defined".to_string(),
            output_excerpt: "duplicate definition for `build`".to_string(),
            ..super::super::repair_job::RepairJob::new_for_test()
        };
        let before = "def build():\n    return 1\n\ndef build():\n    return 2\n";
        let after = "def build():\n    return 1\n\ndef build():\n    return 3\n";

        let err =
            validate_duplicate_binding_repair_candidate("app/main.py", &context, before, after)
                .unwrap_err();

        assert_eq!(
            err.message(),
            "repair intent rejected: duplicate binding still present after candidate edit (build=2)"
        );
    }

    #[test]
    fn duplicate_binding_candidate_accepts_duplicate_reduction() {
        let context = super::super::repair_job::RepairJob {
            failure_signature: "function `build` is already defined".to_string(),
            output_excerpt: "duplicate definition for `build`".to_string(),
            ..super::super::repair_job::RepairJob::new_for_test()
        };
        let before = "def build():\n    return 1\n\ndef build():\n    return 2\n";
        let after = "def build():\n    return 2\n";

        assert!(
            validate_duplicate_binding_repair_candidate("app/main.py", &context, before, after)
                .is_ok()
        );
    }
}
