use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::modes::plan_act::{ExecutionMode, ModeState};
use crate::ollama::xml_fallback::ToolCall;
use crate::session::anvil_score::{AnvilScore, deserialize_lossy_anvil_score};
use crate::session::feedback::{FeedbackFrame, mask_secrets, normalize_path_to_workspace};
use crate::session::precaution::{
    AddPrecautionOutcome, Precaution, PrecautionStatus, RetiredReason, Severity,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct WorkingMemory {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_task: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touched_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_errors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_precautions: Vec<Precaution>,
}

impl WorkingMemory {
    const MAX_CONSTRAINTS: usize = 8;
    const MAX_TOUCHED_FILES: usize = 12;
    const MAX_UNRESOLVED_ERRORS: usize = 8;
    pub const MAX_ACTIVE_PRECAUTIONS: usize = 16;
    pub const MAX_PRECAUTION_TEXT: usize = 240;
    pub const MAX_PRECAUTION_RAW_TEXT: usize = 8 * 1024;
    pub const MAX_PRECAUTIONS_TOTAL: usize = 64;
    /// Cap on `applies_to` length per precaution (CB-002). Bounds canonicalize
    /// / sort / dedup work and `fs::canonicalize` syscalls per add/load.
    pub const MAX_PRECAUTION_APPLIES_TO: usize = 32;
    /// Hard cap on number of `Active Precautions:` bullets emitted into the
    /// Act-mode prompt (Issue #453). Must be `<= MAX_ACTIVE_PRECAUTIONS`.
    pub const MAX_ACTIVE_PRECAUTIONS_PROMPT: usize = 8;
    /// Soft cap on cumulative `chars().count()` of bullet lines emitted into
    /// the `Active Precautions:` section. At least one precaution is always
    /// included even if its single line exceeds this cap (Issue #453).
    pub const MAX_ACTIVE_PRECAUTIONS_CHARS: usize = 1024;

    pub fn set_active_task(&mut self, task: Option<String>) {
        self.active_task = task.map(|value| truncate_entry(value, 240));
    }

    pub fn replace_constraints(&mut self, constraints: Vec<String>) {
        self.constraints = constraints
            .into_iter()
            .map(|entry| truncate_entry(entry, 180))
            .take(Self::MAX_CONSTRAINTS)
            .collect();
    }

    pub fn note_touched_file(&mut self, path: String) {
        let path = truncate_entry(path, 180);
        self.touched_files.retain(|entry| entry != &path);
        self.touched_files.push(path);
        while self.touched_files.len() > Self::MAX_TOUCHED_FILES {
            self.touched_files.remove(0);
        }
    }

    pub fn note_error(&mut self, error: String) {
        let error = truncate_entry(error, 220);
        self.unresolved_errors.retain(|entry| entry != &error);
        self.unresolved_errors.push(error);
        while self.unresolved_errors.len() > Self::MAX_UNRESOLVED_ERRORS {
            self.unresolved_errors.remove(0);
        }
    }

    /// Backward-compatible wrapper: render the working memory using the
    /// full Active-only precaution list (no severity sort / no token-budget
    /// cap / no relevance filter). This is the entry point used by:
    /// * existing tests that predate Issue #453;
    /// * the Reminder Sidecar (`format_for_prompt()` is called via
    ///   `loop_run::turn::handle_user_message` to build
    ///   `active_precautions_summary`, which needs the full list to keep
    ///   duplicate-push detection precise — see Issue #453 design judgment #5);
    /// * `compact.rs::compact_messages_preserves_active_precautions` regression.
    ///
    /// The selection / sort / cap pipeline for the Act-mode prompt lives in
    /// `loop_run::turn::select_precautions_for_prompt` and reaches the
    /// renderer via [`Self::format_for_prompt_with_precautions`].
    pub fn format_for_prompt(&self) -> Option<String> {
        let active: Vec<Precaution> = self
            .active_precautions
            .iter()
            .filter(|p| p.status == PrecautionStatus::Active)
            .cloned()
            .collect();
        self.format_for_prompt_with_precautions(&active)
    }

    /// Render the working memory using the supplied pre-filtered / sorted /
    /// budgeted precautions snapshot for the `Active Precautions:` section.
    ///
    /// The caller is expected to pre-filter to `status == Active`, but the
    /// renderer re-applies the same filter defensively (Issue #453 S5-001):
    /// any non-Active entry slipped in by mistake is silently skipped instead
    /// of being rendered as a binding constraint.
    ///
    /// # Invariant (CB-002)
    ///
    /// The supplied `Precaution`s must already be sanitized via
    /// [`Self::add_precaution`] / [`Self::sanitize_active_precautions_after_load`].
    /// This renderer does not re-mask secrets, re-truncate text, or re-canonicalize
    /// `applies_to`; bypassing the storage pipeline can leak unmasked content
    /// into the prompt and `llm-io.jsonl`.
    pub fn format_for_prompt_with_precautions(&self, precautions: &[Precaution]) -> Option<String> {
        let active: Vec<&Precaution> = precautions
            .iter()
            .filter(|p| p.status == PrecautionStatus::Active)
            .collect();
        let active_precaution_some = !active.is_empty();
        if self.active_task.is_none()
            && self.constraints.is_empty()
            && self.touched_files.is_empty()
            && self.unresolved_errors.is_empty()
            && !active_precaution_some
        {
            return None;
        }

        let mut lines = vec!["[Working Memory]".to_string()];
        if let Some(task) = &self.active_task {
            lines.push(format!("Active task: {task}"));
        }
        if !self.constraints.is_empty() {
            lines.push("Constraints:".to_string());
            for item in &self.constraints {
                lines.push(format!("- {item}"));
            }
        }
        if !self.touched_files.is_empty() {
            lines.push("Touched files:".to_string());
            for item in &self.touched_files {
                lines.push(format!("- {item}"));
            }
        }
        // Active Precautions section: emitted between Touched files and
        // Unresolved errors so the prompt order is
        // Active task -> Constraints -> Touched files -> Active Precautions ->
        // Unresolved errors (design judgment #9, DR1-006). Severity sort /
        // token-budget filtering / relevance scoring is performed by the
        // caller (`select_precautions_for_prompt` in turn.rs) per Issue #453.
        if active_precaution_some {
            lines.push("Active Precautions:".to_string());
            for p in active {
                lines.push(format!("- [{}] {}", p.severity.as_label(), p.text));
            }
        }
        if !self.unresolved_errors.is_empty() {
            lines.push("Unresolved errors:".to_string());
            for item in &self.unresolved_errors {
                lines.push(format!("- {item}"));
            }
        }
        Some(lines.join("\n"))
    }

    /// Canonical entry point for inserting a `Precaution`. Bypassing this
    /// method skips secret masking, truncation, path normalization, and id
    /// derivation — see module docstring on `precaution.rs`.
    pub fn add_precaution(
        &mut self,
        precaution: Precaution,
        workspace_root: &Path,
    ) -> AddPrecautionOutcome {
        let (canonical, was_truncated) = canonicalize_for_storage(precaution, workspace_root);

        if self.find_blocking_duplicate(&canonical).is_some() {
            return AddPrecautionOutcome::DuplicateIgnored;
        }

        self.evict_oldest_active_if_full();
        self.active_precautions.push(canonical);
        self.prune_precaution_history_if_needed();

        if was_truncated {
            AddPrecautionOutcome::Truncated
        } else {
            AddPrecautionOutcome::Added
        }
    }

    /// Mark an Active precaution as Resolved. Returns true on success.
    pub fn resolve_precaution(&mut self, id: &str) -> bool {
        if let Some(p) = self.active_precautions.iter_mut().find(|p| p.id == id)
            && p.status == PrecautionStatus::Active
        {
            p.status = PrecautionStatus::Resolved;
            return true;
        }
        false
    }

    /// Retire a precaution (terminal). Returns true on success.
    pub fn retire_precaution(&mut self, id: &str) -> bool {
        if let Some(p) = self.active_precautions.iter_mut().find(|p| p.id == id)
            && matches!(
                p.status,
                PrecautionStatus::Active | PrecautionStatus::Resolved
            )
        {
            p.status = PrecautionStatus::Retired;
            p.retired_reason = Some(RetiredReason::UserRetired);
            return true;
        }
        false
    }

    /// Re-apply the canonicalization pipeline to every loaded precaution
    /// using bounded reconstruction (CB-001 / design judgment #14).
    ///
    /// session.json is user-editable, so the on-disk Vec is not trusted. The
    /// sanitizer rebuilds `active_precautions` from scratch:
    ///   * each entry is re-canonicalized (mask / truncate / path normalize /
    ///     id recompute / Unknown-status downgrade);
    ///   * Active entries pass through `add_precaution`-equivalent duplicate
    ///     detection and FIFO active-cap eviction so a tampered file with 50
    ///     identical-id Active entries collapses to one and overflow Active
    ///     entries are evicted to `Retired(CapacityEvicted)`;
    ///   * Resolved / Retired entries are de-duplicated by id;
    ///   * the final length is bounded by `prune_precaution_history_if_needed`
    ///     and a fail-safe `truncate(MAX_PRECAUTIONS_TOTAL)` so even a file
    ///     dominated by Active or by Retired-with-no-victim-reason entries
    ///     cannot exceed the total cap.
    pub fn sanitize_active_precautions_after_load(&mut self, workspace_root: &Path) {
        let raw = std::mem::take(&mut self.active_precautions);
        for precaution in raw {
            let canonical = canonicalize_loaded_precaution(precaution, workspace_root);
            match canonical.status {
                PrecautionStatus::Active => {
                    if self.find_blocking_duplicate(&canonical).is_some() {
                        // Duplicate id (vs. Active / Resolved / blocked Retired)
                        // — drop instead of pushing.
                        continue;
                    }
                    self.evict_oldest_active_if_full();
                    self.active_precautions.push(canonical);
                }
                PrecautionStatus::Resolved | PrecautionStatus::Retired => {
                    if !self.active_precautions.iter().any(|p| p.id == canonical.id) {
                        self.active_precautions.push(canonical);
                    }
                }
                PrecautionStatus::Unknown => {
                    // canonicalize_loaded_precaution downgrades Unknown to
                    // Retired(Unknown) so this branch is unreachable in
                    // practice, but kept defensively to make the invariant
                    // explicit.
                    if !self.active_precautions.iter().any(|p| p.id == canonical.id) {
                        self.active_precautions.push(canonical);
                    }
                }
            }
        }
        self.prune_precaution_history_if_needed();
        // Final fail-safe — even if every entry survived pruning (e.g. the
        // file consists entirely of Active entries up to the active cap plus
        // Retired entries that were not pruning victims), guarantee the total
        // cap. This terminal truncate is the last line of defence (CB-001).
        if self.active_precautions.len() > Self::MAX_PRECAUTIONS_TOTAL {
            self.active_precautions
                .truncate(Self::MAX_PRECAUTIONS_TOTAL);
        }
    }

    /// id-based duplicate detection with status × retired_reason matrix
    /// (DR1-004). `(Retired, None)` and `(Retired, Some(Unknown))` are
    /// conservatively treated as blocking to honor old session.json semantics.
    fn find_blocking_duplicate(&self, p: &Precaution) -> Option<&Precaution> {
        self.active_precautions.iter().find(|existing| {
            existing.id == p.id
                && match (existing.status, existing.retired_reason) {
                    (PrecautionStatus::Active, _) => true,
                    (PrecautionStatus::Resolved, _) => true,
                    (PrecautionStatus::Retired, Some(RetiredReason::UserRetired)) => true,
                    (PrecautionStatus::Retired, Some(RetiredReason::CapacityEvicted)) => false,
                    (PrecautionStatus::Retired, Some(RetiredReason::Unknown)) => true,
                    (PrecautionStatus::Retired, None) => true,
                    (PrecautionStatus::Unknown, _) => true,
                }
        })
    }

    /// FIFO-evict the oldest Active precaution into Retired(CapacityEvicted)
    /// when at MAX_ACTIVE_PRECAUTIONS capacity. The CapacityEvicted reason
    /// allows the same canonical key to be re-Activated next time
    /// `add_precaution` is called (design judgment #3).
    fn evict_oldest_active_if_full(&mut self) {
        let active_count = self
            .active_precautions
            .iter()
            .filter(|p| p.status == PrecautionStatus::Active)
            .count();
        if active_count < Self::MAX_ACTIVE_PRECAUTIONS {
            return;
        }
        if let Some(idx) = self
            .active_precautions
            .iter()
            .position(|p| p.status == PrecautionStatus::Active)
        {
            let evicted = &mut self.active_precautions[idx];
            evicted.status = PrecautionStatus::Retired;
            evicted.retired_reason = Some(RetiredReason::CapacityEvicted);
        }
    }

    /// Bound the total Vec length at `MAX_PRECAUTIONS_TOTAL` (design judgment
    /// #13). Pruning order:
    ///   1. Retired(CapacityEvicted)
    ///   2. Resolved
    ///   3. Retired(UserRetired)
    ///   4. Retired(Unknown) / Retired(None) — added in CB-001 so a tampered
    ///      file consisting entirely of these states cannot bypass the total
    ///      cap by hitting the previous fall-through `break`.
    ///
    /// Active items are never pruned here — Active capacity is enforced by
    /// `evict_oldest_active_if_full`. Final fail-safe is in
    /// `sanitize_active_precautions_after_load`.
    fn prune_precaution_history_if_needed(&mut self) {
        while self.active_precautions.len() > Self::MAX_PRECAUTIONS_TOTAL {
            let victim = self
                .active_precautions
                .iter()
                .position(|p| {
                    p.status == PrecautionStatus::Retired
                        && p.retired_reason == Some(RetiredReason::CapacityEvicted)
                })
                .or_else(|| {
                    self.active_precautions
                        .iter()
                        .position(|p| p.status == PrecautionStatus::Resolved)
                })
                .or_else(|| {
                    self.active_precautions.iter().position(|p| {
                        p.status == PrecautionStatus::Retired
                            && p.retired_reason == Some(RetiredReason::UserRetired)
                    })
                })
                .or_else(|| {
                    // CB-001: Retired(Unknown) / Retired(None) entries are also
                    // valid pruning victims. Without this branch a tampered
                    // session.json full of these states would break out of
                    // the loop and leave total > MAX_PRECAUTIONS_TOTAL.
                    self.active_precautions.iter().position(|p| {
                        p.status == PrecautionStatus::Retired
                            && !matches!(
                                p.retired_reason,
                                Some(RetiredReason::CapacityEvicted)
                                    | Some(RetiredReason::UserRetired)
                            )
                    })
                });
            if let Some(idx) = victim {
                self.active_precautions.remove(idx);
            } else {
                break;
            }
        }
    }
}

/// Pre-storage canonicalization: raw cap -> sanitize_text -> mask -> truncate
/// -> path normalize/sort/dedup -> severity-unknown downgrade -> id derivation.
/// Returns `(precaution, was_truncated)` where `was_truncated` is whether
/// `text` exceeded `MAX_PRECAUTION_TEXT` after `mask_secrets` was applied.
fn canonicalize_for_storage(mut p: Precaution, workspace_root: &Path) -> (Precaution, bool) {
    // 1. text: raw cap -> control-char strip / newline-fold (CB-006) -> mask
    //    -> truncate (DR1-002 / DR2-006 / DR4-002).
    p.text = truncate_entry(p.text, WorkingMemory::MAX_PRECAUTION_RAW_TEXT);
    p.text = sanitize_text(&p.text);
    p.text = mask_secrets(&p.text);
    let original_chars = p.text.chars().count();
    p.text = truncate_entry(p.text, WorkingMemory::MAX_PRECAUTION_TEXT);
    let truncated = original_chars > WorkingMemory::MAX_PRECAUTION_TEXT;

    // 2. applies_to: reject ParentDir + workspace-relativize + sort/dedup by
    //    canonical key + cap input length (CB-002).
    p.applies_to = canonicalize_applies_to(&p.applies_to, workspace_root);

    // 3. severity: collapse forward-compat `Unknown` to `Medium` (CB-003) so
    //    the prompt uses a known label and id derivation does not depend on a
    //    label the runtime cannot interpret. Severity does not feed into the
    //    id hash, so this is safe ordering-wise.
    if matches!(p.severity, Severity::Unknown) {
        p.severity = Severity::Medium;
    }

    // 4. id: SHA-256 full hex over (text, source, applies_to canonical sorted).
    p.id = compute_precaution_id(&p);
    p.status = PrecautionStatus::Active;
    p.retired_reason = None;

    (p, truncated)
}

/// CB-006: collapse newlines (`\n` / `\r`) to a single space and drop other
/// ASCII / Unicode control characters. Keeps regular spaces, tabs are
/// converted to spaces (tabs are control chars in `char::is_control`). This
/// runs before `mask_secrets` so a multi-line payload cannot smuggle a forged
/// `Unresolved errors:` section header into the prompt.
fn sanitize_text(input: &str) -> String {
    input
        .chars()
        .filter_map(|c| match c {
            '\n' | '\r' | '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

/// Defensive re-canonicalization for precautions read from session.json.
/// Preserves the existing status/retired_reason but recomputes the id from
/// canonical fields. `Unknown` status is downgraded to `Retired(Unknown)` so it
/// is never prompt-injected as Active (design judgment #14).
fn canonicalize_loaded_precaution(p: Precaution, workspace_root: &Path) -> Precaution {
    let original_status = p.status;
    let original_retired_reason = p.retired_reason;
    let (mut canonical, _) = canonicalize_for_storage(p, workspace_root);
    canonical.status = match original_status {
        PrecautionStatus::Active => PrecautionStatus::Active,
        PrecautionStatus::Resolved => PrecautionStatus::Resolved,
        PrecautionStatus::Retired => PrecautionStatus::Retired,
        PrecautionStatus::Unknown => PrecautionStatus::Retired,
    };
    canonical.retired_reason = if canonical.status == PrecautionStatus::Retired {
        if original_status == PrecautionStatus::Unknown {
            Some(RetiredReason::Unknown)
        } else {
            original_retired_reason.or(Some(RetiredReason::Unknown))
        }
    } else {
        None
    };
    canonical
}

/// SHA-256 full hex of `text + 0x00 + source_tag + 0x00 + applies_to_keys`.
/// Storage uses 64-hex full hash; CLI tooling (#454/#455) is free to display
/// the leading 12-hex prefix (design judgment #5).
fn compute_precaution_id(p: &Precaution) -> String {
    let mut hasher = Sha256::new();
    hasher.update(p.text.as_bytes());
    hasher.update(b"\x00");
    hasher.update(p.source.as_label().as_bytes());
    hasher.update(b"\x00");
    for path in &p.applies_to {
        if let Some(key) = path_to_canonical_string(path) {
            hasher.update(key.as_bytes());
            hasher.update(b"\x00");
        }
    }
    format!("{:x}", hasher.finalize())
}

/// Canonical "/"-joined UTF-8 string of a path, OR None if any component is
/// non-UTF-8. We deliberately never emit a partial path (DR4-004): a single
/// non-UTF-8 component drops the whole path.
fn path_to_canonical_string(p: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = p.components().map(|c| c.as_os_str().to_str()).collect();
    parts.map(|parts| parts.join("/"))
}

/// Sanitize `applies_to` for hashing/display: cap input length, reject `..`
/// (twofold defence against path traversal: explicit reject +
/// `normalize_path_to_workspace`), drop non-UTF-8 paths, drop absolute paths
/// that do not resolve inside `workspace_root`, drop relative paths that fall
/// back to a bare basename, and sort + dedup by canonical key
/// (DR1-001 / DR1-007 / CB-002 / CB-004).
fn canonicalize_applies_to(input: &[PathBuf], workspace_root: &Path) -> Vec<PathBuf> {
    let mut keyed: Vec<(String, PathBuf)> = input
        .iter()
        // CB-002: cap input length so a tampered or huge applies_to cannot
        // amplify canonicalize / sort / dedup work.
        .take(WorkingMemory::MAX_PRECAUTION_APPLIES_TO)
        .filter(|p| {
            !p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        })
        .filter_map(|p| canonicalize_single_path_in_workspace(p, workspace_root))
        .filter_map(|p| {
            let key = path_to_canonical_string(&p)?;
            (!key.is_empty()).then_some((key, p))
        })
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    keyed.dedup_by(|a, b| a.0 == b.0);
    keyed.into_iter().map(|(_, p)| p).collect()
}

/// CB-004: stricter wrapper around `normalize_path_to_workspace` for the
/// precaution store.
///
/// `normalize_path_to_workspace` falls back to `Path::file_name()` when
/// canonicalize fails, which would let an absolute path outside the workspace
/// (`/etc/passwd`) or a multi-component nonexistent path leak as a bare
/// basename. The precaution layer is stricter: it must never store a path
/// whose original meaning was outside the workspace.
///
/// Policy:
///   * absolute path -> require `canonicalize` to land inside `workspace_root`
///     (any other outcome drops the path);
///   * relative path -> use `normalize_path_to_workspace`, but if the helper
///     reduces a multi-component path to a bare basename via `file_name()`
///     fallback (i.e. canonicalize failed and the original had > 1 component
///     or differs from the basename), drop it.
fn canonicalize_single_path_in_workspace(p: &Path, workspace_root: &Path) -> Option<PathBuf> {
    if p.is_absolute() {
        let abs = p.canonicalize().ok()?;
        let root = workspace_root.canonicalize().ok()?;
        let rel = abs.strip_prefix(&root).ok()?;
        return Some(rel.to_path_buf());
    }
    let normalized = normalize_path_to_workspace(p, workspace_root)?;
    // Detect basename-only fallback: if the original path had more than one
    // component but the helper returned a single-component bare basename,
    // canonicalize must have failed and the helper fell back to `file_name`.
    // We refuse that (CB-004) so a nonexistent `nested/dir/leak.rs` does not
    // collapse to `leak.rs` in storage.
    if normalized.components().count() == 1
        && p.components().count() > 1
        && p.file_name().map(PathBuf::from).as_deref() == Some(normalized.as_path())
        && p.canonicalize().is_err()
    {
        return None;
    }
    Some(normalized)
}

pub(crate) fn truncate_entry(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value;
    }
    let truncated = value.chars().take(max_chars).collect::<String>();
    format!("{truncated}...")
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ConversationMessage {
    pub fn system(content: String) -> Self {
        Self {
            role: "system".to_string(),
            content,
            tool_calls: Vec::new(),
            name: None,
        }
    }

    pub fn user(content: String) -> Self {
        Self {
            role: "user".to_string(),
            content,
            tool_calls: Vec::new(),
            name: None,
        }
    }

    pub fn assistant(content: String, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
            tool_calls,
            name: None,
        }
    }

    pub fn tool(name: String, content: String) -> Self {
        Self {
            role: "tool".to_string(),
            content,
            tool_calls: Vec::new(),
            name: Some(name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct SessionSnapshot {
    pub mode_state: ModeState,
    pub messages: Vec<ConversationMessage>,
    pub checkpoints: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_root: Option<PathBuf>,
    #[serde(default)]
    pub native_tools_disabled: bool,
    #[serde(default)]
    pub working_memory: WorkingMemory,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub workspace_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_feedback: Option<FeedbackFrame>,
    /// Issue #455 / D4 / CB-001: turn-scoped flag for "an eligible-kind
    /// FeedbackFrame has already been recorded during the current turn".
    /// `record_feedback` automatically sets it whenever an eligible frame
    /// is written; `record_feedback_if_unset` consults it and skips when
    /// already set; `reset_eligible_recorded_this_turn` clears it at turn
    /// boundaries (callers invoke this at the top of `run_turn`).
    ///
    /// Not persisted (`#[serde(skip)]`): this is purely runtime state. Old
    /// session.json files load with the field defaulted to `false`, which
    /// is the correct turn-start value for a freshly resumed session.
    #[serde(skip, default)]
    pub eligible_feedback_recorded_this_turn: bool,
    /// Issue #456: persisted snapshot of the previous turn's AnvilScore. Used
    /// by [`crate::session::anvil_score::compute_anvil_score`] as the delta
    /// baseline for the current turn. `None` for fresh sessions and for the
    /// pre-compute window inside a turn (DR1-010 / 設計判断 #9).
    ///
    /// Field-level lossy deserializer (`deserialize_lossy_anvil_score`) drops
    /// malformed / oversized JSON to `None` so a tampered session.json or an
    /// old-anvil future-incompatible field cannot block resume / discovery
    /// (DR1-008 / DR4-003 / S5-004 / S7-003).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_lossy_anvil_score"
    )]
    pub last_anvil_score: Option<AnvilScore>,
    /// Issue #456: turn-local counter for `UnsafeCommandBlocked` events. Reset
    /// to `0` at the top of `run_turn`, incremented at the unsafe-block
    /// `record_feedback` site, copied into `AnvilScore.unsafe_actions_blocked`
    /// at compute time. Not persisted.
    #[serde(skip, default)]
    pub unsafe_blocks_this_turn: usize,
    /// Issue #456: session-cumulative counter for `NoRepoProgress` turns.
    /// Incremented when the post-loop `verify_repo_progress` finds no diff,
    /// reset to 0 when the turn produces verifiable progress. Persisted so
    /// resumed sessions keep their accumulated streak.
    #[serde(default)]
    pub consecutive_no_progress_turns: usize,
    /// Issue #456: turn-local flag set when at least one `Write` / `Edit`
    /// tool call returned `Ok`. Reset to `false` at the top of `run_turn`,
    /// consumed by `compute_anvil_score` to determine `user_visible_artifact`.
    /// Not persisted.
    #[serde(skip, default)]
    pub repo_edit_succeeded_this_turn: bool,
    /// Issue #456: snapshot of `working_memory.touched_files` captured at the
    /// top of `run_turn`. Provides the Reminder Sidecar / future Observability
    /// with a stable view of "what the agent already knew before this turn",
    /// independent of the in-turn cap that `working_memory.touched_files`
    /// applies. Not persisted (this is per-turn diagnostic context, not state
    /// to resume from). Open Question OQ-1: removal candidate if no consumer
    /// emerges.
    #[serde(skip, default)]
    pub touched_files_at_turn_start: Vec<String>,
    /// Issue #462: turn-local per-turn cap for CaseRecord extraction. Set to
    /// `true` once `maybe_extract_case_record` runs (regardless of
    /// extraction outcome), preventing duplicate work in the same turn. Not
    /// persisted; runtime-only flag like `repo_edit_succeeded_this_turn`.
    #[serde(skip, default)]
    pub case_record_extracted_this_turn: bool,
    /// Issue #463: turn-local per-turn cap for case_retrieval injection.
    /// Set to `true` once `try_inject_case_retrieval_message` consumes the
    /// cap (env disable / failure / completed / dry-run / below-threshold).
    /// Plan-mode early return does NOT set this. Reset at `run_turn` head.
    #[serde(skip, default)]
    pub case_retrieval_invoked_this_turn: bool,
}

impl SessionSnapshot {
    /// Overwrite `last_feedback` with the given frame. Updates the
    /// turn-scoped flag (`eligible_feedback_recorded_this_turn`) when the
    /// frame's kind is Reminder-eligible so subsequent
    /// [`Self::record_feedback_if_unset`] calls in the same turn correctly
    /// skip. The same-turn override rule (design 5.5) is preserved for the
    /// last_feedback field itself: later calls within a turn still
    /// overwrite `last_feedback`.
    pub fn record_feedback(&mut self, frame: FeedbackFrame) {
        if frame.kind.is_eligible_for_reminder() {
            self.eligible_feedback_recorded_this_turn = true;
        }
        self.last_feedback = Some(frame);
    }

    /// Issue #455 / D4 / CB-001: first-eligible-failure-wins guard. Skips
    /// recording when `eligible_feedback_recorded_this_turn` is already
    /// `true`; otherwise records `frame` via [`Self::record_feedback`].
    ///
    /// CB-001: this method intentionally uses the in-snapshot bool flag
    /// rather than comparing frame contents against a baseline snapshot.
    /// Identical failure frames (e.g. NoToolCall with the same static
    /// reason, or `deterministic_content_fallback` whose `primary_error` is
    /// a fixed string) recurring across turns would compare equal under
    /// value semantics and silently bypass the guard.
    ///
    /// Single-event sites (Bash failure / edit failure inside tool dispatch)
    /// keep using [`Self::record_feedback`] because last-write-wins is the
    /// right semantics for them — the flag still propagates so later
    /// guarded sites in the same turn see "an eligible failure already
    /// fired".
    pub fn record_feedback_if_unset(&mut self, frame: FeedbackFrame) {
        if self.eligible_feedback_recorded_this_turn {
            return;
        }
        self.record_feedback(frame);
    }

    /// Issue #455 / D4 / CB-001: clear the turn-scoped flag. MUST be
    /// invoked at the top of `run_turn` so the in-snapshot baseline starts
    /// fresh on every turn.
    pub fn reset_eligible_feedback_recorded_this_turn(&mut self) {
        self.eligible_feedback_recorded_this_turn = false;
    }

    /// Issue #462: clear the per-turn CaseRecord extraction flag. Invoked from
    /// `run_turn` head alongside the other turn-local resets so the next turn
    /// can extract afresh.
    pub fn reset_case_record_extracted_this_turn(&mut self) {
        self.case_record_extracted_this_turn = false;
    }
}

pub struct SessionStore {
    path: PathBuf,
    state_root: PathBuf,
    session_id: String,
    workspace_key: String,
}

impl SessionStore {
    pub fn new(state_root: &Path, session_id: &str, workspace_key: &str) -> Self {
        let path = state_root
            .join("sessions")
            .join(session_id)
            .join("session.json");
        Self {
            path,
            state_root: state_root.to_path_buf(),
            session_id: session_id.to_string(),
            workspace_key: workspace_key.to_string(),
        }
    }

    /// Load a session.json from disk (or return a fresh `SessionSnapshot`).
    ///
    /// IMPORTANT (CB-005): the returned `SessionSnapshot` is **untrusted**.
    /// session.json is user-editable and may contain stale ids, unmasked
    /// secrets, oversized text, traversal paths, unknown enum variants, and
    /// active-precaution counts beyond `WorkingMemory::MAX_ACTIVE_PRECAUTIONS`.
    /// Callers MUST invoke
    /// `WorkingMemory::sanitize_active_precautions_after_load(&workspace_root)`
    /// on `snapshot.working_memory` before passing the snapshot to
    /// `WorkingMemory::format_for_prompt`, before re-saving via
    /// [`SessionStore::save`], or before exposing it to the agent loop. The
    /// canonical entry point in `src/lib.rs` does this immediately after
    /// `reconcile_resume_state`; do not skip it on alternate code paths.
    pub fn load_or_new(&self, fresh: bool) -> Result<SessionSnapshot, String> {
        if fresh || !self.path.exists() {
            return Ok(SessionSnapshot {
                id: self.session_id.clone(),
                workspace_key: self.workspace_key.clone(),
                ..SessionSnapshot::default()
            });
        }

        // Issue #456 / DR4-003: refuse to read oversized session.json before
        // we even open it so a hostile / corrupt file cannot push the parser
        // into an oversized allocation. The cap mirrors the discovery-path
        // limit in `discovery::MAX_SESSION_JSON_BYTES` so resume and
        // `iter_session_dirs` enforce the same upper bound.
        if let Ok(file_meta) = fs::metadata(&self.path)
            && file_meta.len() > crate::session::discovery::MAX_SESSION_JSON_BYTES
        {
            return Err(format!(
                "session {} exceeds {} bytes",
                self.path.display(),
                crate::session::discovery::MAX_SESSION_JSON_BYTES
            ));
        }

        let contents = fs::read_to_string(&self.path)
            .map_err(|err| format!("failed to read session {}: {err}", self.path.display()))?;
        let mut snapshot: SessionSnapshot = serde_json::from_str(&contents)
            .map_err(|err| format!("failed to parse session {}: {err}", self.path.display()))?;
        if snapshot.id.is_empty() {
            snapshot.id = self.session_id.clone();
        }
        if snapshot.workspace_key.is_empty() {
            snapshot.workspace_key = self.workspace_key.clone();
        }
        Ok(snapshot)
    }

    pub fn save(&self, session: &SessionSnapshot) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
        }
        let mut snapshot = session.clone();
        if snapshot.id.is_empty() {
            snapshot.id = self.session_id.clone();
        }
        if snapshot.workspace_key.is_empty() {
            snapshot.workspace_key = self.workspace_key.clone();
        }
        let contents = serde_json::to_string_pretty(&snapshot)
            .map_err(|err| format!("failed to serialize session: {err}"))?;
        fs::write(&self.path, contents)
            .map_err(|err| format!("failed to write session {}: {err}", self.path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600));
        }

        Ok(())
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn log_dir(&self) -> PathBuf {
        self.state_root
            .join("sessions")
            .join(&self.session_id)
            .join("logs")
    }

    pub fn plan_dir(&self) -> PathBuf {
        self.state_root
            .join("sessions")
            .join(&self.session_id)
            .join("plans")
    }

    pub fn log_dir_for(&self, session_id: &str) -> Option<PathBuf> {
        if uuid::Uuid::parse_str(session_id).is_err() {
            return None;
        }
        let candidate = self
            .state_root
            .join("sessions")
            .join(session_id)
            .join("logs");
        if candidate.exists() {
            Some(candidate)
        } else {
            None
        }
    }
}

/// Bring a restored `SessionSnapshot` back to a runnable state before it is
/// handed to `Agent::new`:
///   (1) If `active_root` points at a directory that no longer exists,
///       clear it so the agent falls back to the current cwd.
///   (2) If the session is mid-Plan but its `active_plan_path` file is gone,
///       downgrade to Act so the agent does not try to open a missing plan.
///
/// Emits a `warn:` line to stderr for each reconciliation. Idempotent.
pub fn reconcile_resume_state(session: &mut SessionSnapshot, _cwd: &Path) {
    if let Some(root) = &session.active_root
        && !root.is_dir()
    {
        eprintln!(
            "warn: session.active_root no longer exists ({}); falling back to cwd",
            root.display()
        );
        session.active_root = None;
    }

    if session.mode_state.mode == ExecutionMode::Plan {
        let missing_plan = session
            .mode_state
            .active_plan_path
            .as_ref()
            .is_some_and(|p| !p.exists());
        if missing_plan {
            eprintln!(
                "warn: plan file missing ({}); downgrading to Act mode",
                session
                    .mode_state
                    .active_plan_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            );
            session.mode_state.mode = ExecutionMode::Act;
            session.mode_state.active_plan_path = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::session::precaution::PrecautionSource;

    use super::{Precaution, PrecautionStatus, Severity, WorkingMemory, compute_precaution_id};
    use tempfile::tempdir;

    fn make_precaution(text: &str, severity: Severity, status: PrecautionStatus) -> Precaution {
        Precaution {
            id: format!("manual-{text}"),
            source: PrecautionSource::Manual,
            severity,
            text: text.to_string(),
            applies_to: Vec::new(),
            status,
            retired_reason: None,
        }
    }

    #[test]
    fn compute_precaution_id_uses_stable_source_label() {
        let precaution = Precaution {
            id: String::new(),
            source: PrecautionSource::ToolFailure,
            severity: Severity::Medium,
            text: "legacy-id-fixture".to_string(),
            applies_to: Vec::new(),
            status: PrecautionStatus::Active,
            retired_reason: None,
        };

        assert_eq!(
            compute_precaution_id(&precaution),
            "672e482bcd99c2d07445e5acce743b3ec669b2e7916e0af59c9ef6a668e4e573"
        );
    }

    #[test]
    fn format_for_prompt_with_precautions_renders_supplied_list() {
        // Issue #453: format_for_prompt_with_precautions must render exactly
        // what the caller supplies, ignoring `self.active_precautions`.
        let dir = tempdir().unwrap();
        let mut wm = WorkingMemory::default();
        // self.active_precautions has one entry that should NOT appear.
        wm.add_precaution(
            make_precaution(
                "self-side-only-do-not-render",
                Severity::Medium,
                PrecautionStatus::Active,
            ),
            dir.path(),
        );

        let supplied = vec![make_precaution(
            "supplied-only-render-me",
            Severity::High,
            PrecautionStatus::Active,
        )];

        let rendered = wm
            .format_for_prompt_with_precautions(&supplied)
            .expect("should render");
        assert!(rendered.contains("Active Precautions:"));
        assert!(rendered.contains("- [high] supplied-only-render-me"));
        assert!(
            !rendered.contains("self-side-only-do-not-render"),
            "self.active_precautions must be ignored: {rendered}"
        );
    }

    #[test]
    fn format_for_prompt_with_precautions_filters_non_active_defensively() {
        // Issue #453 S5-001: even if the caller mistakenly passes a
        // Resolved/Retired/Unknown entry, the renderer must drop it.
        let mut wm = WorkingMemory::default();
        wm.set_active_task(Some("dummy".to_string()));
        let supplied = vec![
            make_precaution("active-keep", Severity::High, PrecautionStatus::Active),
            make_precaution("resolved-drop", Severity::High, PrecautionStatus::Resolved),
            make_precaution("retired-drop", Severity::Medium, PrecautionStatus::Retired),
            make_precaution("unknown-drop", Severity::Low, PrecautionStatus::Unknown),
        ];

        let rendered = wm
            .format_for_prompt_with_precautions(&supplied)
            .expect("should render at least the Active task line");
        assert!(rendered.contains("- [high] active-keep"));
        assert!(!rendered.contains("resolved-drop"), "{rendered}");
        assert!(!rendered.contains("retired-drop"), "{rendered}");
        assert!(!rendered.contains("unknown-drop"), "{rendered}");
    }

    #[test]
    fn format_for_prompt_with_precautions_renders_empty_as_no_section() {
        // Issue #453 DR1-003: an empty supplied list must not emit the
        // `Active Precautions:` header. With everything else empty too,
        // the entire working memory message must be None.
        let wm = WorkingMemory::default();
        let rendered = wm.format_for_prompt_with_precautions(&[]);
        assert!(rendered.is_none(), "expected None, got {:?}", rendered);
    }

    /// Issue #461 / DR3-003 / DR2-007 (Sidecar dedup): adding the same
    /// `SafetyPolicy` precaution multiple times with deterministic text
    /// must dedup via id-based blocking-duplicate detection so that
    /// `MAX_ACTIVE_PRECAUTIONS=16` is never breached, and the same
    /// pattern does not push other source precautions out of the cap.
    /// The id derives from (canonicalized text, source, applies_to).
    #[test]
    fn safety_policy_precaution_dedups_by_id_under_repeated_add() {
        let dir = tempdir().unwrap();
        let mut wm = WorkingMemory::default();

        // Pre-fill with 8 distinct non-SafetyPolicy precautions.
        for i in 0..8 {
            wm.add_precaution(
                make_precaution(
                    &format!("other-source-precaution-{i}"),
                    Severity::Medium,
                    PrecautionStatus::Active,
                ),
                dir.path(),
            );
        }
        let baseline_active = wm
            .active_precautions
            .iter()
            .filter(|p| p.status == PrecautionStatus::Active)
            .count();
        assert_eq!(baseline_active, 8);

        // Now hammer the same SafetyPolicy precaution 50 times.
        let safety_text = "blocked dangerous command fragment: shutdown (category=DangerousVerb)";
        for _ in 0..50 {
            let pre = Precaution {
                id: String::new(), // recomputed by canonicalize_for_storage
                source: PrecautionSource::SafetyPolicy,
                severity: Severity::High,
                text: safety_text.to_string(),
                applies_to: Vec::new(),
                status: PrecautionStatus::Active,
                retired_reason: None,
            };
            let _ = wm.add_precaution(pre, dir.path());
        }

        // Cap not breached.
        let total = wm.active_precautions.len();
        assert!(
            total <= WorkingMemory::MAX_ACTIVE_PRECAUTIONS,
            "active precautions {total} exceeds MAX_ACTIVE_PRECAUTIONS"
        );

        // Original 8 non-SafetyPolicy precautions are still Active (not
        // pushed out by the SafetyPolicy onslaught).
        let other_active = wm
            .active_precautions
            .iter()
            .filter(|p| {
                p.status == PrecautionStatus::Active && p.source != PrecautionSource::SafetyPolicy
            })
            .count();
        assert_eq!(
            other_active,
            8,
            "the 8 non-SafetyPolicy active precautions must not be evicted: \
             active = {:?}",
            wm.active_precautions
                .iter()
                .map(|p| (&p.text, p.source, p.status))
                .collect::<Vec<_>>()
        );

        // Exactly one SafetyPolicy entry remains Active (id dedup).
        let safety_active = wm
            .active_precautions
            .iter()
            .filter(|p| {
                p.source == PrecautionSource::SafetyPolicy && p.status == PrecautionStatus::Active
            })
            .count();
        assert_eq!(
            safety_active, 1,
            "deterministic-text SafetyPolicy precaution must dedup to 1 Active entry"
        );
    }

    #[test]
    fn format_for_prompt_with_precautions_renders_empty_section_when_other_fields_present() {
        // When other working-memory fields exist but no active precautions
        // are supplied, the message should still render but without an
        // Active Precautions section.
        let mut wm = WorkingMemory::default();
        wm.set_active_task(Some("do something".to_string()));
        let rendered = wm
            .format_for_prompt_with_precautions(&[])
            .expect("should render with active task only");
        assert!(rendered.contains("Active task: do something"));
        assert!(
            !rendered.contains("Active Precautions:"),
            "section must be hidden: {rendered}"
        );
    }

    // --- Issue #455 / D4: in-snapshot eligible_feedback flag --------------

    use super::SessionSnapshot;
    use crate::session::feedback::{FeedbackFrame, FeedbackKind};

    /// Build a minimal `FeedbackFrame` with the given kind through serde so
    /// the unit test does not have to thread `build_feedback_frame` (the
    /// guard is purely about kind, not field content).
    fn frame_with_kind(kind: &str) -> FeedbackFrame {
        let json = format!("{{\"kind\":\"{kind}\"}}");
        serde_json::from_str(&json).expect("frame deserializes")
    }

    /// Issue #455 / D4 / CB-001: when this turn already recorded a Reminder-
    /// eligible failure frame (via `record_feedback`), a follow-up call to
    /// `record_feedback_if_unset` must be a no-op (first eligible failure
    /// wins).
    #[test]
    fn record_feedback_if_unset_no_op_when_this_turn_failure_set() {
        let mut snap = SessionSnapshot::default();
        // Fire 1: this turn records a TestFailure (sets the flag).
        snap.record_feedback(frame_with_kind("test_failure"));
        assert!(
            snap.eligible_feedback_recorded_this_turn,
            "record_feedback of failure-kind must set the flag"
        );
        // Fire 2: NoToolCall would overwrite — guard must block it.
        snap.record_feedback_if_unset(frame_with_kind("no_tool_call"));
        assert_eq!(
            snap.last_feedback.as_ref().unwrap().kind,
            FeedbackKind::TestFailure,
            "first this-turn failure-kind frame must win"
        );
    }

    /// Issue #455 / CB-001: identical failure frames (e.g. NoToolCall with
    /// the same static reason) recurring across turns must NOT bypass the
    /// guard. With the bool flag, the previous turn's leftover frame in
    /// `last_feedback` is irrelevant — only the in-snapshot flag gates.
    #[test]
    fn record_feedback_if_unset_records_when_flag_unset_even_with_stale_failure() {
        let stale = frame_with_kind("test_failure");
        let mut snap = SessionSnapshot {
            last_feedback: Some(stale),
            // new turn baseline: flag starts false (after reset)
            eligible_feedback_recorded_this_turn: false,
            ..SessionSnapshot::default()
        };
        snap.record_feedback_if_unset(frame_with_kind("no_tool_call"));
        assert_eq!(
            snap.last_feedback.unwrap().kind,
            FeedbackKind::NoToolCall,
            "previous-turn failure must not block the new turn's record"
        );
        assert!(
            snap.eligible_feedback_recorded_this_turn,
            "fresh eligible record sets the flag"
        );
    }

    /// Pass-kind frames (BuildPass / TestPass) recorded via `record_feedback`
    /// earlier in the turn must NOT set the flag, so later guarded sites
    /// can still record. They are semantically "no-failure" markers.
    #[test]
    fn record_feedback_if_unset_records_when_only_pass_kind_recorded() {
        let mut snap = SessionSnapshot::default();
        // Fire 1: pass-kind (BuildPass) lands during this turn — flag stays false.
        snap.record_feedback(frame_with_kind("build_pass"));
        assert!(
            !snap.eligible_feedback_recorded_this_turn,
            "pass-kind must not set the flag"
        );
        // Fire 2: NoToolCall must overwrite it AND set the flag.
        snap.record_feedback_if_unset(frame_with_kind("no_tool_call"));
        assert_eq!(
            snap.last_feedback.unwrap().kind,
            FeedbackKind::NoToolCall,
            "pass-kind frames must not block subsequent record"
        );
        assert!(snap.eligible_feedback_recorded_this_turn);
    }

    /// `last_feedback == None` and the flag false is the standard turn-start
    /// state (after `reset_eligible_feedback_recorded_this_turn`). The new
    /// frame is recorded.
    #[test]
    fn record_feedback_if_unset_records_when_flag_unset_and_field_none() {
        let mut snap = SessionSnapshot::default();
        snap.record_feedback_if_unset(frame_with_kind("no_tool_call"));
        assert_eq!(snap.last_feedback.unwrap().kind, FeedbackKind::NoToolCall);
        assert!(snap.eligible_feedback_recorded_this_turn);
    }

    /// `record_feedback` keeps last-write-wins semantics on `last_feedback`
    /// itself: Bash failure → unsafe block in the same iteration still
    /// overwrites. The flag stays set throughout the turn so subsequent
    /// `record_feedback_if_unset` calls correctly skip.
    #[test]
    fn record_feedback_overwrites_last_value_and_keeps_flag_set() {
        let mut snap = SessionSnapshot::default();
        snap.record_feedback(frame_with_kind("timeout"));
        snap.record_feedback(frame_with_kind("compile_error"));
        assert_eq!(snap.last_feedback.unwrap().kind, FeedbackKind::CompileError);
        assert!(snap.eligible_feedback_recorded_this_turn);
    }

    /// `reset_eligible_feedback_recorded_this_turn` clears the flag without
    /// touching `last_feedback`. After reset, `record_feedback_if_unset`
    /// records again even if `last_feedback` still holds the previous
    /// turn's failure.
    #[test]
    fn reset_eligible_feedback_recorded_this_turn_clears_flag_only() {
        let mut snap = SessionSnapshot::default();
        snap.record_feedback(frame_with_kind("test_failure"));
        assert!(snap.eligible_feedback_recorded_this_turn);
        snap.reset_eligible_feedback_recorded_this_turn();
        assert!(!snap.eligible_feedback_recorded_this_turn);
        assert!(snap.last_feedback.is_some(), "last_feedback preserved");
        // New turn: another record_feedback_if_unset succeeds.
        snap.record_feedback_if_unset(frame_with_kind("no_tool_call"));
        assert_eq!(snap.last_feedback.unwrap().kind, FeedbackKind::NoToolCall);
    }

    /// The flag is `#[serde(skip)]`: serializing and deserializing a
    /// SessionSnapshot drops the runtime flag (resumed sessions start a
    /// turn cleanly).
    #[test]
    fn eligible_feedback_flag_is_not_persisted() {
        let mut snap = SessionSnapshot::default();
        snap.record_feedback(frame_with_kind("test_failure"));
        assert!(snap.eligible_feedback_recorded_this_turn);
        let json = serde_json::to_string(&snap).unwrap();
        assert!(
            !json.contains("eligible_feedback_recorded_this_turn"),
            "flag must be skipped during serialization: {json}"
        );
        let decoded: SessionSnapshot = serde_json::from_str(&json).unwrap();
        assert!(
            !decoded.eligible_feedback_recorded_this_turn,
            "flag must default to false on deserialize"
        );
        assert_eq!(
            decoded.last_feedback.as_ref().unwrap().kind,
            FeedbackKind::TestFailure,
            "last_feedback survives the round-trip"
        );
    }
}
