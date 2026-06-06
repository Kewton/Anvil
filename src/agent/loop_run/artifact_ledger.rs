//! Issue #659: `ArtifactLedger` SSOT for current-turn artifact observations.
//!
//! `ArtifactLedger` aggregates artifact evidence observed during the current
//! user turn so that downstream projections (`turn_edited_relative_paths`,
//! `task_contract_artifact_states`, `owned_test_artifacts_for_verifier`) stop
//! reconstructing the same artifact facts independently.
//!
//! ## Visibility (DR3-001)
//!
//! This module is intentionally **private** — `loop_run.rs` declares it as
//! `mod artifact_ledger;` and never re-exports any name (`pub use`/`pub(crate)
//! use` are forbidden). `turn.rs` is the only in-crate consumer via
//! `super::artifact_ledger::*`.
//!
//! ## Security invariants
//!
//! - Every `record_*` API takes a `LedgerAdmissionContext { work_root, scope }`
//!   so admission goes through the workspace-relative / `..` / control-char
//!   / symlink-escape / ignored-dir checks delegated to
//!   `artifact_ownership::classify_ownership` (the SSOT).
//! - LLM-derived `expected_role` is re-confirmed via `util::file_classify`
//!   (`is_test_file` / `is_setup_file`) and any mismatch is downgraded so the
//!   ledger never promotes a mismatched path to `Owned`.
//! - Observability emit goes through `crate::logging::log_llm_event`, which
//!   re-applies `mask_payload_inplace` as the final-defence pass. Raw paths
//!   are never put in payloads: only `path_hash` (over `mask_secrets`'d
//!   workspace-relative path) and `path_len` are emitted.
//!
//! ## Policy isolation
//!
//! The module deliberately does **not** import any policy types
//! (`CompletionDecision`, `ArtifactRecoveryAction`, `EffectiveToolPolicy`,
//! `RepairJob`, `VerifierCommand`). Projection consumers translate ledger
//! observations into policy decisions on their side.
//!
//! ## Bounded design
//!
//! - `MAX_ARTIFACT_LEDGER_EVENTS = 256` caps the number of accepted events
//!   per turn. When the cap is exceeded, **new** events are rejected — the
//!   already-accepted prefix is preserved so legacy projections stay stable
//!   — and `dropped_count` / `overflowed` are updated so completion can be
//!   treated fail-closed by the consumer (Issue #659 §7 / DR1-002).
//! - `MAX_ARTIFACT_LEDGER_PATH_BYTES = 4096` caps the workspace-relative
//!   path byte length. Rationale is POSIX `PATH_MAX` (Linux `limits.h`); this
//!   is intentionally a separate concept from `MAX_ARTIFACT_ACTION_TEXT_BYTES`
//!   (LLM-output free-text cap) and must not auto-track that other constant.
//!
//! Phase 1 (Issue #659) lands this module alongside `loop_run.rs`'s
//! `mod artifact_ledger;` declaration. Phase 2 will wire `turn.rs` to seed
//! events and read projections; until then several items below are dead code
//! per `cargo check`. `#[allow(dead_code)]` annotations point at the exact
//! items the future caller (`turn.rs`) is expected to touch.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use serde_json::json;

use super::artifact_ownership::{
    ArtifactOwnership, OwnershipInputs, classify_ownership,
    nearest_existing_ancestor_within_work_root,
};
use super::task_contract::{ArtifactRole, TaskContract};
use super::task_workspace_scope::TaskWorkspaceScope;
use crate::logging::{log_llm_event, stable_path_hash};
use crate::session::feedback::mask_secrets;
use crate::util::file_classify::{is_setup_file, is_test_file};

/// Per-turn cap on accepted events. New events past this cap are dropped
/// (not FIFO-evicted) so legacy / projection consumers see a stable prefix.
pub(super) const MAX_ARTIFACT_LEDGER_EVENTS: usize = 256;

/// Workspace-relative path byte cap.
///
/// Rationale: POSIX `PATH_MAX = 4096` (Linux `limits.h`).
///
/// **Independence note (DR1-002)**: this constant intentionally has the same
/// numeric value as `artifact_completion_job::MAX_ARTIFACT_ACTION_TEXT_BYTES`
/// but it is a **different concept** — that one caps LLM-output free text;
/// this one caps workspace-relative path strings. The two must not be folded
/// into a shared SSOT; changing one MUST NOT auto-propagate to the other.
pub(super) const MAX_ARTIFACT_LEDGER_PATH_BYTES: usize = 4096;

/// Per-turn cap on accepted verifier observations (CB-003 follow-up).
///
/// `verifier_observations` lives in a secondary `HashMap` keyed by path and
/// was previously unbounded. A structured verifier with many bound paths,
/// or a future caller passing the same path repeatedly, could grow this map
/// without bound and bypass the per-turn ledger event cap. The cap matches
/// `MAX_ARTIFACT_LEDGER_EVENTS` so the two structures have symmetric DoS
/// resistance. Overflow rejects new path insertions (existing entries keep
/// receiving upsert updates so per-turn latest semantics for already-bound
/// paths is preserved) and updates `dropped_count` / `overflowed` together
/// with the event cap so completion projections fail closed.
pub(super) const MAX_ARTIFACT_LEDGER_OBSERVATIONS: usize = 256;

/// Origin of an artifact observation (3 values, mirrors existing vocabulary).
///
/// `#[non_exhaustive]` (DR1-004 OCP): adding a new origin requires the
/// caller `match` arms to add a default — origins are recorded via dedicated
/// `record_*_event` helpers so external callers do not pass this enum
/// directly today, but the attribute keeps the door open for future origins
/// without breaking match exhaustiveness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub(super) enum ArtifactOrigin {
    /// Scope-aware workspace scan / task-contract candidate projection.
    Existing,
    /// Session-tracked scaffold artifact snapshot.
    Scaffold,
    /// Successful Write/Edit tool call (no-op guard already passed upstream).
    RepoEdit,
}

impl ArtifactOrigin {
    fn label(self) -> &'static str {
        match self {
            ArtifactOrigin::Existing => "existing",
            ArtifactOrigin::Scaffold => "scaffold",
            ArtifactOrigin::RepoEdit => "repo_edit",
        }
    }
}

/// Verifier outcome observed for a specific artifact path (3 values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VerifierOutcome {
    Pass,
    Fail,
    NotRun,
}

/// Observation point captured by structured verifier paths only (Issue
/// #659 §2.4 row 4). Legacy / unbound verifier paths do not record per-
/// artifact observations; projections interpret absence as `NotRun`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VerifierObservation {
    pub argv_path_matched: bool,
    pub last_outcome: VerifierOutcome,
}

/// Per-turn admission context. Borrowed at each `record_*` call site so the
/// ledger does not internalize `work_root` / `scope` (no leak via
/// `SessionSnapshot`).
#[derive(Debug, Clone, Copy)]
pub(super) struct LedgerAdmissionContext<'a> {
    pub(super) work_root: &'a Path,
    pub(super) scope: &'a TaskWorkspaceScope,
}

impl<'a> LedgerAdmissionContext<'a> {
    pub(super) fn new(work_root: &'a Path, scope: &'a TaskWorkspaceScope) -> Self {
        Self { work_root, scope }
    }
}

/// Issue #659 PR-001: observability log context for `event_recorded` /
/// `turn_summary` payloads. Stored inside the `ArtifactLedger` itself so
/// existing record-call signatures stay untouched while turn-level dataset
/// consumers (Issue #660 / #661 / #663) can join ledger events with the
/// owning session / turn.
///
/// `session_id` defaults to an empty string and `turn_index` to `0` for
/// tests and code paths that construct an `ArtifactLedger` directly without
/// going through `turn.rs` (the production seed of the context happens in
/// `clear_per_turn_ledger_state_for_turn`, the PR-002 helper that takes the
/// upcoming turn index as an explicit argument).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ArtifactLedgerLogContext {
    pub(super) session_id: String,
    pub(super) turn_index: u32,
}

impl ArtifactLedgerLogContext {
    pub(super) fn new(session_id: impl Into<String>, turn_index: u32) -> Self {
        Self {
            session_id: session_id.into(),
            turn_index,
        }
    }
}

/// Append-only event. `path` is always present (never `Option<String>`); the
/// `ArtifactState::changed(...)` rows that carry `path: None` are skipped at
/// admission time (Issue #659 §3 / DR2-002).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ArtifactLedgerEvent {
    pub path: String,
    pub role: ArtifactRole,
    pub origin: ArtifactOrigin,
    pub ownership: ArtifactOwnership,
    pub edited_this_turn: bool,
    pub post_scaffold_delta: bool,
    pub recorded_at_seq: u32,
}

/// Reason an admission attempt was rejected. Internal — never surfaced as
/// part of `record_*` return values today (`Option<&Event>` collapses
/// rejection to `None`), but kept as a distinct type so the admission rule
/// remains a pure function we can test directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdmissionRejection {
    /// Empty / absolute / `..` / control-char / out-of-scope per
    /// `classify_ownership` (returned `OutOfScope`).
    OutOfScope,
    /// Path exceeded `MAX_ARTIFACT_LEDGER_PATH_BYTES`.
    PathTooLong,
    /// `path: None` row from `ArtifactState::changed(...)`.
    PathMissing,
    /// Per-turn event cap (`MAX_ARTIFACT_LEDGER_EVENTS`) already reached.
    BoundedFull,
}

/// Outcome of a successful admission — the prepared event awaiting append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AdmissionAcceptance {
    path: String,
    role: ArtifactRole,
    ownership: ArtifactOwnership,
}

/// Per-turn ledger. Holds an append-only event vector plus a secondary
/// verifier-observation index keyed by path (DR1-005: events are immutable
/// once appended; verifier observations live in a separate up-to-date map).
#[derive(Debug, Default)]
pub(super) struct ArtifactLedger {
    events: Vec<ArtifactLedgerEvent>,
    verifier_observations: HashMap<String, VerifierObservation>,
    dropped_count: u32,
    overflowed: bool,
    next_seq: u32,
    /// Issue #659 PR-001: observability log context (session_id / turn_index)
    /// propagated into every `event_recorded` / `turn_summary` payload so
    /// dataset consumers can join per-turn (Section 7.1 of design policy).
    log_context: ArtifactLedgerLogContext,
}

/// Summary returned by [`ArtifactLedger::turn_summary`] for end-of-turn
/// observability emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TurnSummary {
    pub event_count: u32,
    pub dropped_count: u32,
    pub overflowed: bool,
}

/// Issue #663 (AD2 / DR1-002 / DR1-009 / DR4-001): forgeability-safe
/// projection of `required_artifacts_completed` plus the ledger's
/// `overflowed` flag.
///
/// Construction is restricted to
/// [`ArtifactLedger::required_artifacts_completed_projection`] — fields are
/// `pub(super)` strictly for in-module test helpers, never written from
/// outside this module. External callers (e.g. `artifact_completion_job`)
/// read via `is_satisfied(role)` / `overflowed()` accessors only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RequiredArtifactsProjection {
    values: BTreeMap<ArtifactRole, bool>,
    overflowed: bool,
}

impl RequiredArtifactsProjection {
    /// Whether the given role has at least one Owned ledger event
    /// recorded for it during the current turn.
    ///
    /// Fail-closed: returns `false` for every role when the ledger has
    /// overflowed (mirrors the ledger projection's CB-002 contract).
    pub(super) fn is_satisfied(&self, role: ArtifactRole) -> bool {
        if self.overflowed {
            return false;
        }
        self.values.get(&role).copied().unwrap_or(false)
    }

    /// Whether the underlying ledger was in `overflowed=true` state when
    /// this projection was built.
    pub(super) fn overflowed(&self) -> bool {
        self.overflowed
    }
}

impl ArtifactLedger {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Per-turn reset. Caller (`turn.rs::handle_user_message` head) must
    /// invoke this alongside the existing `turn_edited_relative_paths.clear()`
    /// / `turn_pre_tool_file_hashes.clear()` (CLAUDE.md per-turn rule).
    ///
    /// The observability `log_context` is intentionally **preserved** across
    /// `clear()` — `turn.rs` calls `set_log_context()` separately so a new
    /// turn's session_id / turn_index are stamped on the per-turn event
    /// stream. Tests that reuse a ledger across virtual turns can call
    /// `set_log_context()` themselves to mirror the production sequencing.
    pub(super) fn clear(&mut self) {
        self.events.clear();
        self.verifier_observations.clear();
        self.dropped_count = 0;
        self.overflowed = false;
        self.next_seq = 0;
    }

    /// Issue #659 PR-001 / PR-002: stamp the observability log context for
    /// the current turn. `turn.rs::clear_per_turn_ledger_state_for_turn`
    /// calls this with `(session_store.session_id(), upcoming_turn_index)`
    /// so subsequent `event_recorded` / `turn_summary` emits carry the
    /// turn-level join keys defined in Section 7.1 of the design policy.
    /// The upcoming index is passed explicitly (rather than read from
    /// `current_turn_index`) so production sequencing — the increment
    /// happens before the reset in `handle_user_message` — is decoupled
    /// from stamp timing.
    pub(super) fn set_log_context(&mut self, ctx: ArtifactLedgerLogContext) {
        self.log_context = ctx;
    }

    /// Read-only accessor for the current observability log context. Test
    /// helpers may use this to confirm the context propagated through a
    /// turn-level wiring path.
    #[allow(dead_code)]
    pub(super) fn log_context(&self) -> &ArtifactLedgerLogContext {
        &self.log_context
    }

    // --- record helpers --------------------------------------------------

    /// `Existing` origin — workspace scan / task-contract candidate
    /// iteration. Idempotent on `(Existing, role, path)`: repeated baseline
    /// seeds during a single turn do NOT append duplicate rows (returns the
    /// already-stored event).
    ///
    /// Issue #661 (iteration-3 Task 3.2): non-verifier admission route —
    /// stays on `NestedTestAdmission::default()` (= `disabled()`).
    pub(super) fn record_existing_event(
        &mut self,
        ctx: &LedgerAdmissionContext<'_>,
        path: String,
        expected_role: ArtifactRole,
    ) -> Option<&ArtifactLedgerEvent> {
        self.record_internal(
            ctx,
            super::artifact_ownership::NestedTestAdmission::default(),
            path,
            expected_role,
            ArtifactOrigin::Existing,
            AdmissionOriginInputs::Existing,
        )
    }

    /// `Scaffold` origin — session-tracked scaffold snapshot delta. Idempotent
    /// on `(Scaffold, role, path)`; `post_scaffold_delta` is recomputed by
    /// the caller before the seed and a duplicate baseline does not produce
    /// multiple rows.
    ///
    /// Issue #661 (iteration-3 Task 3.2): non-verifier admission route —
    /// stays on `NestedTestAdmission::default()` (= `disabled()`).
    pub(super) fn record_scaffold_event(
        &mut self,
        ctx: &LedgerAdmissionContext<'_>,
        path: String,
        expected_role: ArtifactRole,
        post_scaffold_delta: bool,
    ) -> Option<&ArtifactLedgerEvent> {
        self.record_internal(
            ctx,
            super::artifact_ownership::NestedTestAdmission::default(),
            path,
            expected_role,
            ArtifactOrigin::Scaffold,
            AdmissionOriginInputs::Scaffold {
                post_scaffold_delta,
            },
        )
    }

    /// `RepoEdit` origin — non-no-op Write/Edit tool call. Multiple real
    /// edits append multiple events; projections dedupe by `(role, path)`.
    ///
    /// Issue #661 (iteration-3 Task 3.4 / DR2-003 / 判断 #1): verifier-path
    /// SSOT — passes `NestedTestAdmission::enabled()` so the underlying
    /// `classify_ownership` SSOT can promote nested test subdirs (e.g.
    /// `app/tests/foo.py`) to `Owned`. Production callers always pass
    /// `edited_this_turn=true` which independently satisfies
    /// `has_promotion_signal`, so the admission flip is observationally a
    /// no-op for typical edit paths and acts purely as a SSOT-consistency
    /// guard for the 4 propagation routes. Workspace-relative / symlink
    /// containment (CB-001) / ignored_top_dir / role-confirm checks remain
    /// authoritative and unaffected by the flag.
    pub(super) fn record_repo_edit_event(
        &mut self,
        ctx: &LedgerAdmissionContext<'_>,
        path: String,
        expected_role: ArtifactRole,
        edited_this_turn: bool,
    ) -> Option<&ArtifactLedgerEvent> {
        self.record_internal(
            ctx,
            super::artifact_ownership::NestedTestAdmission::enabled(),
            path,
            expected_role,
            ArtifactOrigin::RepoEdit,
            AdmissionOriginInputs::RepoEdit { edited_this_turn },
        )
    }

    /// Update the verifier observation secondary index. Path is admitted via
    /// the same workspace-relative / scope checks as events; out-of-scope
    /// paths return `false`. The observation is upsert (per-turn latest).
    ///
    /// CB-003: the secondary index is capped at
    /// `MAX_ARTIFACT_LEDGER_OBSERVATIONS`. Upserts on already-bound paths
    /// keep working (per-turn latest semantics) so the cap only blocks
    /// **new** path insertions. When a new insertion is rejected the
    /// ledger increments `dropped_count` and raises `overflowed` so the
    /// projection layer treats completion as untrusted (CB-002 fail-closed
    /// semantics).
    ///
    /// Issue #661 (iteration-3 Task 3.4 / DR2-003 / 判断 #1): verifier-path
    /// SSOT — passes `NestedTestAdmission::enabled()` so a nested test path
    /// observed by the structured verifier (e.g. `app/tests/foo.py` argv
    /// element) is admitted into the secondary index for downstream
    /// projection (`owned_test_artifacts` / verifier-binding callsites).
    /// Workspace-relative / symlink containment / ignored_top_dir checks
    /// remain authoritative (see `admit_path_only`).
    pub(super) fn record_verifier_observation(
        &mut self,
        ctx: &LedgerAdmissionContext<'_>,
        path: &str,
        observation: VerifierObservation,
    ) -> bool {
        let Ok(accepted) = admit_path_only(
            ctx,
            super::artifact_ownership::NestedTestAdmission::enabled(),
            path,
        ) else {
            return false;
        };
        if !self.verifier_observations.contains_key(&accepted)
            && self.verifier_observations.len() >= MAX_ARTIFACT_LEDGER_OBSERVATIONS
        {
            self.dropped_count = self.dropped_count.saturating_add(1);
            self.overflowed = true;
            return false;
        }
        self.verifier_observations.insert(accepted, observation);
        true
    }

    fn record_internal(
        &mut self,
        ctx: &LedgerAdmissionContext<'_>,
        nested_test_admission: super::artifact_ownership::NestedTestAdmission,
        path: String,
        expected_role: ArtifactRole,
        origin: ArtifactOrigin,
        origin_specific: AdmissionOriginInputs,
    ) -> Option<&ArtifactLedgerEvent> {
        let admission = match admit_event(
            ctx,
            nested_test_admission,
            &path,
            expected_role,
            origin_specific,
        ) {
            Ok(accepted) => accepted,
            Err(_) => return None,
        };

        // Baseline (Existing / Scaffold) is idempotent on (origin, role, path):
        // repeated contract evaluation must not append duplicates.
        if matches!(origin, ArtifactOrigin::Existing | ArtifactOrigin::Scaffold)
            && let Some(idx) = self.events.iter().position(|e| {
                e.origin == origin && e.role == admission.role && e.path == admission.path
            })
        {
            return self.events.get(idx);
        }

        if self.events.len() >= MAX_ARTIFACT_LEDGER_EVENTS {
            self.dropped_count = self.dropped_count.saturating_add(1);
            self.overflowed = true;
            return None;
        }

        let seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        let edited_this_turn = matches!(
            origin_specific,
            AdmissionOriginInputs::RepoEdit {
                edited_this_turn: true
            }
        );
        let post_scaffold_delta = matches!(
            origin_specific,
            AdmissionOriginInputs::Scaffold {
                post_scaffold_delta: true
            }
        );
        let event = ArtifactLedgerEvent {
            path: admission.path,
            role: admission.role,
            origin,
            ownership: admission.ownership,
            edited_this_turn,
            post_scaffold_delta,
            recorded_at_seq: seq,
        };
        self.events.push(event);
        let appended = self.events.last().expect("just pushed");
        emit_event_recorded(appended, self.overflowed, &self.log_context);
        Some(appended)
    }

    // --- read primitives -------------------------------------------------

    #[allow(dead_code)]
    pub(super) fn events_iter(&self) -> impl Iterator<Item = &ArtifactLedgerEvent> {
        self.events.iter()
    }

    #[allow(dead_code)]
    pub(super) fn events_for_role(
        &self,
        role: ArtifactRole,
    ) -> impl Iterator<Item = &ArtifactLedgerEvent> {
        self.events.iter().filter(move |e| e.role == role)
    }

    #[allow(dead_code)]
    pub(super) fn verifier_observation_for(&self, path: &str) -> Option<&VerifierObservation> {
        self.verifier_observations.get(path)
    }

    pub(super) fn overflowed(&self) -> bool {
        self.overflowed
    }

    pub(super) fn dropped_count(&self) -> u32 {
        self.dropped_count
    }

    pub(super) fn event_count(&self) -> u32 {
        self.events.len() as u32
    }

    /// End-of-turn summary used by `turn.rs` to emit
    /// `agent.artifact_ledger.turn_summary` once per turn.
    pub(super) fn turn_summary(&self) -> TurnSummary {
        TurnSummary {
            event_count: self.event_count(),
            dropped_count: self.dropped_count,
            overflowed: self.overflowed,
        }
    }

    /// Emit `agent.artifact_ledger.turn_summary`. Caller (`turn.rs`) is
    /// responsible for invoking this exactly once per turn at the
    /// SafeStop / Done confirmation point (Phase 2 task 2.2).
    ///
    /// Section 7.1 contract: payload carries `turn_index` / `session_id`
    /// (from the stored `log_context`) plus `event_count` / `dropped_count`
    /// / `overflowed` and per-{origin,role,ownership} counts. Raw paths are
    /// never emitted.
    pub(super) fn emit_turn_summary(&self) {
        let summary = self.turn_summary();
        let mut origin_counts: BTreeMap<&'static str, u32> = BTreeMap::new();
        let mut role_counts: BTreeMap<&'static str, u32> = BTreeMap::new();
        let mut ownership_counts: BTreeMap<&'static str, u32> = BTreeMap::new();
        for event in &self.events {
            *origin_counts.entry(event.origin.label()).or_default() += 1;
            *role_counts.entry(event.role.label()).or_default() += 1;
            *ownership_counts
                .entry(ownership_label(event.ownership))
                .or_default() += 1;
        }
        log_llm_event(
            "agent.artifact_ledger.turn_summary",
            json!({
                "session_id": self.log_context.session_id,
                "turn_index": self.log_context.turn_index,
                "event_count": summary.event_count,
                "dropped_count": summary.dropped_count,
                "overflowed": summary.overflowed,
                "origin_counts": origin_counts,
                "role_counts": role_counts,
                "ownership_counts": ownership_counts,
            }),
        );
    }

    // --- projection anchors (Issue #659 §3.1) ---------------------------

    /// Owned test artifact paths in deterministic order, deduped by path
    /// (Issue #651 SSOT-equivalent shape).
    pub(super) fn owned_test_artifacts(&self, role: ArtifactRole) -> Vec<String> {
        projection::owned_test_artifacts(self, role)
    }

    /// Issue #659 (Task 3.1): deterministic `BTreeSet<String>` projection of
    /// every `RepoEdit`-origin event's path. This is the SSOT-equivalent
    /// view of the legacy `Agent::turn_edited_relative_paths` HashSet used
    /// by the dual-source divergence assertion (Task 2.7) and by the
    /// Phase 3 internal-implementation switch of `task_contract_artifact_states`
    /// / `owned_test_artifacts_for_verifier`. Paths are NOT filtered by
    /// ownership — the legacy set carries every successfully observed edit
    /// regardless of `classify_ownership` so the projection must mirror
    /// that semantics during the adapter period.
    pub(super) fn repo_edit_projection_set(&self) -> BTreeSet<String> {
        self.events
            .iter()
            .filter(|ev| matches!(ev.origin, ArtifactOrigin::RepoEdit))
            .map(|ev| ev.path.clone())
            .collect()
    }

    /// Per-required-role completion view in deterministic
    /// `BTreeMap<ArtifactRole, bool>` iteration order (anchor; full
    /// arbitration is Issue #663 scope).
    pub(super) fn required_artifacts_completed(
        &self,
        contract: &TaskContract,
    ) -> BTreeMap<ArtifactRole, bool> {
        projection::required_artifacts_completed(self, contract)
    }

    /// Issue #663 (AD2 / DR1-002 / DR1-009 / DR4-001): forgeability-safe
    /// projection wrapping the per-role completion view + the ledger's
    /// `overflowed` flag. The constructor lives in this module only —
    /// external callers cannot forge a `RequiredArtifactsProjection` and
    /// must use this accessor.
    ///
    /// Fail-closed contract: when the ledger has overflowed, `is_satisfied`
    /// returns `false` for every role and `overflowed()` returns `true`.
    pub(super) fn required_artifacts_completed_projection(
        &self,
        contract: &TaskContract,
    ) -> RequiredArtifactsProjection {
        let values = projection::required_artifacts_completed(self, contract);
        RequiredArtifactsProjection {
            values,
            overflowed: self.overflowed,
        }
    }

    /// Active job candidate roles. Anchor stub returning declaration order
    /// (full priority arbitration is Issue #660 scope).
    pub(super) fn active_job_candidates(&self, contract: &TaskContract) -> Vec<ArtifactRole> {
        projection::active_job_candidates(self, contract)
    }
}

// ---------------------------------------------------------------------------
// Admission rule (pure functions, kept private to this module)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum AdmissionOriginInputs {
    Existing,
    Scaffold { post_scaffold_delta: bool },
    RepoEdit { edited_this_turn: bool },
}

/// Pure admission rule. Validates `path` against the security gates
/// (workspace-relative / `..` / control-char / length cap / scope /
/// symlink-escape) by re-using `classify_ownership` (the SSOT), then
/// reconfirms `role` via `util::file_classify` so an LLM-derived
/// `expected_role` cannot promote a mismatched path to `Owned`.
///
/// CB-001 hardening: `classify_ownership`'s canonical-escape check returns
/// `false` for missing leaves, so a not-yet-created file under a symlinked
/// ancestor (e.g. `tests/new_test.py` where `tests -> /outside/...`) would
/// pass admission and become completion evidence. We re-apply the nearest-
/// existing-ancestor containment check (`artifact_ownership::
/// nearest_existing_ancestor_within_work_root` — PR-002 SSOT) so missing-
/// leaf rows go through exactly the same workspace confinement gate as the
/// `artifact_completion_job` write target.
///
/// Issue #661 (iteration-3 Task 3.2 / 3.4 / DR2-003): `nested_test_admission`
/// is threaded through to `classify_ownership` so the 4 verifier-path SSOT
/// callers (`record_repo_edit_event` / `record_verifier_observation` /
/// `validate_bound_test_artifacts_for_execution` /
/// `seed_artifact_ledger_verifier_observation`) can opt into recognising
/// nested test subdirs (e.g. `app/tests/foo.py`) as `Owned`. Every other
/// caller passes `NestedTestAdmission::default()` (= `disabled()`),
/// preserving the legacy reject behaviour exactly. The flag never bypasses
/// the workspace-relative / symlink containment / ignored_top_dir /
/// role-confirm checks — those gates remain authoritative.
fn admit_event(
    ctx: &LedgerAdmissionContext<'_>,
    nested_test_admission: super::artifact_ownership::NestedTestAdmission,
    path: &str,
    expected_role: ArtifactRole,
    origin_specific: AdmissionOriginInputs,
) -> Result<AdmissionAcceptance, AdmissionRejection> {
    if path.is_empty() {
        return Err(AdmissionRejection::PathMissing);
    }
    if path.len() > MAX_ARTIFACT_LEDGER_PATH_BYTES {
        return Err(AdmissionRejection::PathTooLong);
    }
    let inputs = OwnershipInputs {
        work_root: ctx.work_root,
        relative_path: path,
        scope: ctx.scope,
        edited_this_session: matches!(
            origin_specific,
            AdmissionOriginInputs::RepoEdit {
                edited_this_turn: true
            }
        ),
        scaffold_changed: matches!(
            origin_specific,
            AdmissionOriginInputs::Scaffold {
                post_scaffold_delta: true
            }
        ),
        verifier_passed_in_scope: false,
        nested_test_admission,
    };
    let ownership = classify_ownership(inputs);
    if matches!(ownership, ArtifactOwnership::OutOfScope) {
        return Err(AdmissionRejection::OutOfScope);
    }
    // CB-001: re-check nearest-existing-ancestor containment so a missing
    // leaf under a symlinked ancestor cannot become completion evidence.
    if !ancestor_within_work_root(ctx.work_root, path) {
        return Err(AdmissionRejection::OutOfScope);
    }
    // Reconfirm role from path. `expected_role` is treated as a hint; a
    // path-classifier mismatch downgrades ownership so the row cannot
    // promote to Owned via the wrong-role channel.
    let final_ownership = if role_matches_path(expected_role, path) {
        ownership
    } else {
        match ownership {
            ArtifactOwnership::Owned => ArtifactOwnership::CandidateOnly,
            other => other,
        }
    };
    Ok(AdmissionAcceptance {
        path: path.to_string(),
        role: expected_role,
        ownership: final_ownership,
    })
}

/// Path-only admission (no role / origin). Used by
/// `record_verifier_observation` to share the workspace / scope / path
/// validation pipeline without producing an `ArtifactLedgerEvent`.
///
/// Issue #661 (iteration-3 Task 3.2 / DR2-003): the same
/// `nested_test_admission` SSOT propagation rule as `admit_event` applies;
/// see its doc comment for the per-caller contract.
fn admit_path_only(
    ctx: &LedgerAdmissionContext<'_>,
    nested_test_admission: super::artifact_ownership::NestedTestAdmission,
    path: &str,
) -> Result<String, AdmissionRejection> {
    if path.is_empty() {
        return Err(AdmissionRejection::PathMissing);
    }
    if path.len() > MAX_ARTIFACT_LEDGER_PATH_BYTES {
        return Err(AdmissionRejection::PathTooLong);
    }
    let inputs = OwnershipInputs {
        work_root: ctx.work_root,
        relative_path: path,
        scope: ctx.scope,
        edited_this_session: false,
        scaffold_changed: false,
        verifier_passed_in_scope: false,
        nested_test_admission,
    };
    if matches!(classify_ownership(inputs), ArtifactOwnership::OutOfScope) {
        return Err(AdmissionRejection::OutOfScope);
    }
    // CB-001: missing leaves still need the ancestor containment check —
    // see `admit_event` doc comment.
    if !ancestor_within_work_root(ctx.work_root, path) {
        return Err(AdmissionRejection::OutOfScope);
    }
    Ok(path.to_string())
}

/// Wrap `nearest_existing_ancestor_within_work_root` so callers do not have
/// to re-join `work_root` at every site. Returns `true` when the leaf (or
/// nearest existing ancestor) canonicalizes inside `work_root`.
fn ancestor_within_work_root(work_root: &Path, relative_path: &str) -> bool {
    let candidate = work_root.join(relative_path);
    nearest_existing_ancestor_within_work_root(work_root, &candidate)
}

fn role_matches_path(role: ArtifactRole, path: &str) -> bool {
    let p = Path::new(path);
    match role {
        // Test: must look like a test file.
        ArtifactRole::Test => is_test_file(p),
        // Setup: must look like a setup / config file.
        ArtifactRole::Setup => is_setup_file(p),
        // Implementation / UsageDocs: classifier overlaps with other roles
        // (e.g. `.md` for UsageDocs has no dedicated predicate today). We
        // accept the caller's expected_role *unless* the path is obviously
        // a test/setup file claiming to be impl/docs — that mismatch is the
        // injection vector we guard against here.
        ArtifactRole::Implementation => !is_test_file(p) && !is_setup_file(p),
        ArtifactRole::UsageDocs => !is_test_file(p) && !is_setup_file(p),
        ArtifactRole::DataOutput => matches!(
            p.extension().and_then(|ext| ext.to_str()),
            Some("csv" | "tsv" | "jsonl" | "ndjson" | "parquet")
        ),
    }
}

fn ownership_label(o: ArtifactOwnership) -> &'static str {
    match o {
        ArtifactOwnership::Owned => "owned",
        ArtifactOwnership::CandidateOnly => "candidate_only",
        ArtifactOwnership::OutOfScope => "out_of_scope",
    }
}

// ---------------------------------------------------------------------------
// Observability hook (per Issue #659 §7.1)
// ---------------------------------------------------------------------------

fn emit_event_recorded(
    event: &ArtifactLedgerEvent,
    overflowed: bool,
    ctx: &ArtifactLedgerLogContext,
) {
    // path_hash is derived from the *masked* workspace-relative path so a
    // secret accidentally embedded in the path can never surface via the
    // hash. Raw path is NEVER included in the payload.
    let masked = mask_secrets(&event.path);
    let path_hash = stable_path_hash(&masked);
    log_llm_event(
        "agent.artifact_ledger.event_recorded",
        json!({
            "session_id": ctx.session_id,
            "turn_index": ctx.turn_index,
            "seq": event.recorded_at_seq,
            "origin": event.origin.label(),
            "role": event.role.label(),
            "ownership": ownership_label(event.ownership),
            "edited_this_turn": event.edited_this_turn,
            "post_scaffold_delta": event.post_scaffold_delta,
            "path_hash": path_hash,
            "path_len": event.path.len() as u32,
            "overflowed": overflowed,
        }),
    );
}

// Issue #661 DR1-002 / DR2-005: `stable_path_hash` was previously a private
// duplicate in this module; it has been promoted to `crate::logging::
// stable_path_hash` so all three former duplicate sites (this module +
// `turn.rs::stable_path_hash_for_active_job` + `active_job_arbiter.rs`
// `#[cfg(test)]` helper) share one SSOT and `agent.artifact_ledger.*`
// event `path_hash` values remain a stable correlator with `agent.
// active_job.selected` / `agent.verifier.invoked` payloads. Imported via
// `use crate::logging::stable_path_hash;` at the top of this module so
// in-module call sites (e.g. `emit_event_recorded`, `bounded_masked_path_hashes`)
// resolve to the SSOT directly. Issue #666 `job_report.rs` consumers also
// route through the same `crate::logging::stable_path_hash` SSOT.

/// Issue #659 PR-001: bounded, masked path-hash projection helper. Each
/// path is passed through `mask_secrets` and then `stable_path_hash` so the
/// hash space matches `event_recorded.path_hash` exactly (dataset consumers
/// can join the divergence list back to per-event rows). Output is hard-
/// capped at `MAX_DIVERGENCE_PATH_HASHES = 16` entries to bound payload
/// size — beyond the cap, additional paths are silently dropped (counts
/// still reflect the true totals).
pub(super) const MAX_DIVERGENCE_PATH_HASHES: usize = 16;

/// Build a bounded list of masked path hashes for divergence payloads.
/// Accepts any `IntoIterator<Item = &str>` so call sites can pass a
/// `BTreeSet<String>` iterator (deterministic order) without intermediate
/// allocation.
pub(super) fn bounded_masked_path_hashes<'a, I>(paths: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    paths
        .into_iter()
        .take(MAX_DIVERGENCE_PATH_HASHES)
        .map(|p| stable_path_hash(&mask_secrets(p)))
        .collect()
}

// ---------------------------------------------------------------------------
// Projection (Issue #659 §3.1 — `mod projection` per DR1-001 SRP)
// ---------------------------------------------------------------------------

mod projection {
    use super::*;

    /// Owned artifact paths for `role` in deterministic order, deduped by
    /// path. Mirrors `artifact_ownership::owned_test_artifacts` semantics for
    /// `ArtifactRole::Test`; generalises to any role by reading the same
    /// ownership classification stored at admission time.
    ///
    /// CB-002 fail-closed: when the ledger has overflowed (`dropped_count >
    /// 0` / `overflowed == true`), the ledger is no longer a trustworthy
    /// SSOT for completion / verifier binding — we may have dropped
    /// `Owned` events on the floor. Return an empty list so the caller
    /// (verifier binding, `OwnedTestVerifierPlan` selection) cannot mark
    /// a turn as `Runnable` from a partial ledger view.
    pub(super) fn owned_test_artifacts(ledger: &ArtifactLedger, role: ArtifactRole) -> Vec<String> {
        if ledger.overflowed {
            return Vec::new();
        }
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut out: Vec<String> = Vec::new();
        for event in &ledger.events {
            if event.role != role {
                continue;
            }
            if !matches!(event.ownership, ArtifactOwnership::Owned) {
                continue;
            }
            if seen.insert(event.path.as_str()) {
                out.push(event.path.clone());
            }
        }
        out
    }

    /// Per-required-role completion view. Minimal implementation: a role is
    /// "completed" when at least one ledger event for that role has
    /// `ownership == Owned`. Full arbitration is Issue #663 scope.
    ///
    /// CB-002 fail-closed: when the ledger has overflowed, every role is
    /// reported as `false`. A turn cannot claim completion from a partial
    /// ledger projection.
    pub(super) fn required_artifacts_completed(
        ledger: &ArtifactLedger,
        contract: &TaskContract,
    ) -> BTreeMap<ArtifactRole, bool> {
        let mut out: BTreeMap<ArtifactRole, bool> = BTreeMap::new();
        let overflowed = ledger.overflowed;
        for role in &contract.required_artifacts {
            let completed = !overflowed
                && ledger
                    .events
                    .iter()
                    .any(|e| e.role == *role && matches!(e.ownership, ArtifactOwnership::Owned));
            out.insert(*role, completed);
        }
        out
    }

    /// Stub: returns the declaration order of `contract.required_artifacts`.
    /// Issue #660 will implement the full priority-ordered arbitration.
    ///
    /// CB-002 fail-closed: when the ledger has overflowed, return an empty
    /// candidate set so the caller (Issue #660 arbitration) cannot promote
    /// any role to `active` from a partial ledger view.
    pub(super) fn active_job_candidates(
        ledger: &ArtifactLedger,
        contract: &TaskContract,
    ) -> Vec<ArtifactRole> {
        if ledger.overflowed {
            return Vec::new();
        }
        contract.required_artifacts.clone()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::task_workspace_scope::{ScopeMode, TaskWorkspaceScope};
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    // Issue #920 (DR4-001): accepting the `data_output` LLM role-label via
    // `from_label` must NOT let a non-data path be admitted as a DataOutput
    // artifact. `role_matches_path` is the Tier-B path-local admission guard and
    // stays exhaustive (no `_ =>`): a DataOutput role only matches a data file.
    #[test]
    fn data_output_role_cannot_admit_non_data_path() {
        // Non-data paths must NOT be admitted as DataOutput, even though the
        // role label now round-trips through from_label.
        assert!(!role_matches_path(ArtifactRole::DataOutput, "src/main.py"));
        assert!(!role_matches_path(ArtifactRole::DataOutput, "app/main.py"));
        assert!(!role_matches_path(
            ArtifactRole::DataOutput,
            "tests/test_x.py"
        ));
        assert!(!role_matches_path(ArtifactRole::DataOutput, "README.md"));
        // Genuine data files are admitted.
        assert!(role_matches_path(ArtifactRole::DataOutput, "out.csv"));
        assert!(role_matches_path(
            ArtifactRole::DataOutput,
            "data/records.jsonl"
        ));
    }

    fn single_root_scope() -> TaskWorkspaceScope {
        TaskWorkspaceScope {
            mode: ScopeMode::SingleProjectRoot,
        }
    }

    fn ctx<'a>(work_root: &'a Path, scope: &'a TaskWorkspaceScope) -> LedgerAdmissionContext<'a> {
        LedgerAdmissionContext::new(work_root, scope)
    }

    /// Minimal `RequiredBehaviorContract` for projection tests. The struct
    /// intentionally does not implement `Default` (Issue #635 DR2-006), so
    /// each test that needs one constructs it explicitly with the smallest
    /// schema we can fit.
    fn test_required_behavior() -> super::super::required_behavior::RequiredBehaviorContract {
        super::super::required_behavior::RequiredBehaviorContract {
            operations: None,
            domain_terms: None,
            interface_hints: None,
            required_artifacts: None,
            verification: None,
            confidence: 0.0,
            test_execution_required: false,
            // Issue #665: explicit None per DR2-006 (literal site policy).
            behavior_goal: None,
            required_capabilities: None,
            verification_expectations: None,
            non_goals: None,
        }
    }

    fn test_contract(
        required_artifacts: Vec<ArtifactRole>,
        verification_required: bool,
    ) -> super::super::task_contract::TaskContract {
        let required_behavior = test_required_behavior();
        let intent = super::super::task_contract::TaskIntent::Build;
        let task_kind = super::super::task_contract::TaskKind::Coding;
        let completion_policy = super::super::task_contract::CompletionPolicy::from_contract_parts(
            task_kind,
            intent,
            &required_artifacts,
            verification_required,
            &required_behavior,
        );
        let deliverables = required_artifacts
            .iter()
            .copied()
            .map(|role| super::super::task_contract::TaskDeliverable {
                kind: match role {
                    ArtifactRole::Implementation => {
                        super::super::task_contract::DeliverableKind::Code
                    }
                    ArtifactRole::Test => super::super::task_contract::DeliverableKind::Tests,
                    ArtifactRole::UsageDocs => {
                        super::super::task_contract::DeliverableKind::UsageDocs
                    }
                    ArtifactRole::Setup => super::super::task_contract::DeliverableKind::Setup,
                    ArtifactRole::DataOutput => {
                        super::super::task_contract::DeliverableKind::StructuredRecord
                    }
                },
                role: Some(role),
                path: None,
                required_sections: Vec::new(),
            })
            .collect();
        super::super::task_contract::TaskContract {
            task_kind,
            intent,
            deliverables,
            required_artifacts,
            required_artifact_identities: vec![],
            optional_artifacts: vec![],
            verification_required,
            completion_policy,
            required_behavior,
            // Issue #917: synthetic test contract — neutral matched confidence.
            classification_confidence: 1.0,
            evidence_command_hint: None,
        }
    }

    // ---- Task 1.1: skeleton -----------------------------------------------

    #[test]
    fn constants_match_design_policy() {
        assert_eq!(MAX_ARTIFACT_LEDGER_EVENTS, 256);
        assert_eq!(MAX_ARTIFACT_LEDGER_PATH_BYTES, 4096);
    }

    #[test]
    fn ledger_starts_empty() {
        let ledger = ArtifactLedger::new();
        assert_eq!(ledger.event_count(), 0);
        assert_eq!(ledger.dropped_count(), 0);
        assert!(!ledger.overflowed());
        assert_eq!(ledger.events_iter().count(), 0);
    }

    #[test]
    fn clear_resets_state() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("tests/test_a.py"), "").ok();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("tests/test_a.py"), "").unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        ledger.record_repo_edit_event(
            &ctx(dir.path(), &scope),
            "tests/test_a.py".to_string(),
            ArtifactRole::Test,
            true,
        );
        assert_eq!(ledger.event_count(), 1);
        ledger.record_verifier_observation(
            &ctx(dir.path(), &scope),
            "tests/test_a.py",
            VerifierObservation {
                argv_path_matched: true,
                last_outcome: VerifierOutcome::Pass,
            },
        );
        assert!(ledger.verifier_observation_for("tests/test_a.py").is_some());

        ledger.clear();
        assert_eq!(ledger.event_count(), 0);
        assert_eq!(ledger.dropped_count(), 0);
        assert!(!ledger.overflowed());
        assert_eq!(ledger.events_iter().count(), 0);
        assert!(
            ledger.verifier_observation_for("tests/test_a.py").is_none(),
            "verifier observation secondary index must also be cleared (DR1-005)"
        );
    }

    // ---- Task 1.2: admission rule ----------------------------------------

    #[test]
    fn admit_rejects_absolute_path() {
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        let err = admit_event(
            &ctx(dir.path(), &scope),
            super::super::artifact_ownership::NestedTestAdmission::default(),
            "/etc/passwd",
            ArtifactRole::Implementation,
            AdmissionOriginInputs::Existing,
        )
        .unwrap_err();
        assert_eq!(err, AdmissionRejection::OutOfScope);
    }

    #[test]
    fn admit_rejects_parent_dir() {
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        let err = admit_event(
            &ctx(dir.path(), &scope),
            super::super::artifact_ownership::NestedTestAdmission::default(),
            "../sibling/x.py",
            ArtifactRole::Implementation,
            AdmissionOriginInputs::Existing,
        )
        .unwrap_err();
        assert_eq!(err, AdmissionRejection::OutOfScope);
    }

    #[test]
    fn admit_rejects_control_char() {
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        let err = admit_event(
            &ctx(dir.path(), &scope),
            super::super::artifact_ownership::NestedTestAdmission::default(),
            "tests/bad\u{0007}.py",
            ArtifactRole::Test,
            AdmissionOriginInputs::Existing,
        )
        .unwrap_err();
        assert_eq!(err, AdmissionRejection::OutOfScope);
    }

    #[test]
    fn admit_rejects_ignored_top_dir() {
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        let err = admit_event(
            &ctx(dir.path(), &scope),
            super::super::artifact_ownership::NestedTestAdmission::default(),
            "node_modules/foo/index.js",
            ArtifactRole::Implementation,
            AdmissionOriginInputs::RepoEdit {
                edited_this_turn: true,
            },
        )
        .unwrap_err();
        assert_eq!(err, AdmissionRejection::OutOfScope);
    }

    #[test]
    fn admit_rejects_too_long_path() {
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        let big = "a/".repeat(MAX_ARTIFACT_LEDGER_PATH_BYTES) + "x.py";
        let err = admit_event(
            &ctx(dir.path(), &scope),
            super::super::artifact_ownership::NestedTestAdmission::default(),
            &big,
            ArtifactRole::Implementation,
            AdmissionOriginInputs::Existing,
        )
        .unwrap_err();
        assert_eq!(err, AdmissionRejection::PathTooLong);
    }

    #[test]
    fn admit_role_mismatch_does_not_promote_to_owned() {
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        // `expected_role = Test` but path is clearly not a test file.
        let acc = admit_event(
            &ctx(dir.path(), &scope),
            super::super::artifact_ownership::NestedTestAdmission::default(),
            "src/lib.rs",
            ArtifactRole::Test,
            AdmissionOriginInputs::RepoEdit {
                edited_this_turn: true,
            },
        )
        .unwrap();
        assert_ne!(
            acc.ownership,
            ArtifactOwnership::Owned,
            "role-mismatch must not promote to Owned (LLM role injection guard)"
        );
    }

    #[cfg(unix)]
    #[test]
    fn admit_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let outside = tempdir().unwrap();
        std::fs::write(outside.path().join("secret.py"), "").unwrap();
        let work = tempdir().unwrap();
        symlink(
            outside.path().join("secret.py"),
            work.path().join("alias.py"),
        )
        .unwrap();
        let scope = single_root_scope();
        let err = admit_event(
            &ctx(work.path(), &scope),
            super::super::artifact_ownership::NestedTestAdmission::default(),
            "alias.py",
            ArtifactRole::Implementation,
            AdmissionOriginInputs::RepoEdit {
                edited_this_turn: true,
            },
        )
        .unwrap_err();
        assert_eq!(err, AdmissionRejection::OutOfScope);
    }

    // ---- Task 1.3: record_*_event ----------------------------------------

    #[test]
    fn existing_baseline_is_idempotent() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "").unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        ledger.record_existing_event(
            &ctx(dir.path(), &scope),
            "README.md".to_string(),
            ArtifactRole::UsageDocs,
        );
        ledger.record_existing_event(
            &ctx(dir.path(), &scope),
            "README.md".to_string(),
            ArtifactRole::UsageDocs,
        );
        ledger.record_existing_event(
            &ctx(dir.path(), &scope),
            "README.md".to_string(),
            ArtifactRole::UsageDocs,
        );
        assert_eq!(
            ledger.event_count(),
            1,
            "Existing baseline must be idempotent on (origin, role, path)"
        );
    }

    #[test]
    fn scaffold_baseline_is_idempotent() {
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        ledger.record_scaffold_event(
            &ctx(dir.path(), &scope),
            "app/main.py".to_string(),
            ArtifactRole::Implementation,
            false,
        );
        ledger.record_scaffold_event(
            &ctx(dir.path(), &scope),
            "app/main.py".to_string(),
            ArtifactRole::Implementation,
            true, // even if the caller flips the delta, baseline stays single
        );
        assert_eq!(ledger.event_count(), 1);
    }

    #[test]
    fn repo_edit_appends_per_observation() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("tests/test_a.py"), "").unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        for _ in 0..3 {
            ledger.record_repo_edit_event(
                &ctx(dir.path(), &scope),
                "tests/test_a.py".to_string(),
                ArtifactRole::Test,
                true,
            );
        }
        assert_eq!(
            ledger.event_count(),
            3,
            "RepoEdit appends per non-no-op observation; projection dedupes later"
        );
    }

    #[test]
    fn bounded_rejects_new_events_when_exceeded() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        // Saturate with RepoEdit events of distinct synthetic paths so the
        // baseline-idempotency path does not kick in (RepoEdit is not
        // deduped at admission).
        for i in 0..MAX_ARTIFACT_LEDGER_EVENTS {
            let p = format!("tests/test_{}.py", i);
            std::fs::write(dir.path().join(&p), "").ok();
            ledger.record_repo_edit_event(&ctx(dir.path(), &scope), p, ArtifactRole::Test, true);
        }
        assert_eq!(ledger.event_count() as usize, MAX_ARTIFACT_LEDGER_EVENTS);
        // One more event must be dropped, not FIFO-evicted.
        let overflow_path = "tests/test_overflow.py".to_string();
        std::fs::write(dir.path().join(&overflow_path), "").ok();
        let r = ledger.record_repo_edit_event(
            &ctx(dir.path(), &scope),
            overflow_path,
            ArtifactRole::Test,
            true,
        );
        assert!(r.is_none());
        assert_eq!(ledger.event_count() as usize, MAX_ARTIFACT_LEDGER_EVENTS);
        assert_eq!(ledger.dropped_count(), 1);
        assert!(ledger.overflowed());
    }

    #[test]
    fn verifier_observation_updates_secondary_index() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("tests/test_a.py"), "").unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        assert!(ledger.record_verifier_observation(
            &ctx(dir.path(), &scope),
            "tests/test_a.py",
            VerifierObservation {
                argv_path_matched: true,
                last_outcome: VerifierOutcome::Pass,
            },
        ));
        assert_eq!(
            ledger.verifier_observation_for("tests/test_a.py"),
            Some(&VerifierObservation {
                argv_path_matched: true,
                last_outcome: VerifierOutcome::Pass,
            })
        );
        // Out-of-scope path: rejected, no insertion.
        assert!(!ledger.record_verifier_observation(
            &ctx(dir.path(), &scope),
            "/etc/passwd",
            VerifierObservation {
                argv_path_matched: false,
                last_outcome: VerifierOutcome::NotRun,
            },
        ));
        assert!(ledger.verifier_observation_for("/etc/passwd").is_none());
    }

    // ---- Task 1.4: projection --------------------------------------------

    #[test]
    fn owned_test_artifacts_returns_only_owned_test_paths() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("tests/test_a.py"), "").unwrap();
        std::fs::write(dir.path().join("tests/test_b.py"), "").unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "").ok();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "").unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        ledger.record_repo_edit_event(
            &ctx(dir.path(), &scope),
            "tests/test_a.py".to_string(),
            ArtifactRole::Test,
            true,
        );
        ledger.record_repo_edit_event(
            &ctx(dir.path(), &scope),
            "tests/test_b.py".to_string(),
            ArtifactRole::Test,
            true,
        );
        ledger.record_repo_edit_event(
            &ctx(dir.path(), &scope),
            "src/lib.rs".to_string(),
            ArtifactRole::Implementation,
            true,
        );

        let owned = ledger.owned_test_artifacts(ArtifactRole::Test);
        assert!(owned.contains(&"tests/test_a.py".to_string()));
        assert!(owned.contains(&"tests/test_b.py".to_string()));
        assert!(!owned.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn projection_dedupes_repo_edit_by_role_and_path() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("tests/test_a.py"), "").unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        for _ in 0..5 {
            ledger.record_repo_edit_event(
                &ctx(dir.path(), &scope),
                "tests/test_a.py".to_string(),
                ArtifactRole::Test,
                true,
            );
        }
        let owned = ledger.owned_test_artifacts(ArtifactRole::Test);
        assert_eq!(
            owned,
            vec!["tests/test_a.py".to_string()],
            "projection must dedupe by (role, path)"
        );
    }

    #[test]
    fn required_artifacts_completed_returns_btreemap_ordered() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("tests/test_a.py"), "").unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        ledger.record_repo_edit_event(
            &ctx(dir.path(), &scope),
            "tests/test_a.py".to_string(),
            ArtifactRole::Test,
            true,
        );
        let contract = test_contract(vec![ArtifactRole::Implementation, ArtifactRole::Test], true);
        let completed = ledger.required_artifacts_completed(&contract);
        assert_eq!(completed.get(&ArtifactRole::Test).copied(), Some(true));
        assert_eq!(
            completed.get(&ArtifactRole::Implementation).copied(),
            Some(false)
        );
        // BTreeMap iteration order is the enum's PartialOrd order, which is
        // declaration order: Implementation < Test < UsageDocs < Setup.
        let keys: Vec<_> = completed.keys().copied().collect();
        assert_eq!(keys, vec![ArtifactRole::Implementation, ArtifactRole::Test]);
    }

    #[test]
    fn active_job_candidates_returns_declaration_order_stub() {
        let ledger = ArtifactLedger::new();
        let contract = test_contract(
            vec![ArtifactRole::Implementation, ArtifactRole::Test],
            false,
        );
        let got = ledger.active_job_candidates(&contract);
        assert_eq!(
            got,
            vec![ArtifactRole::Implementation, ArtifactRole::Test],
            "Issue #659 stub returns declaration order; #660 will arbitrate"
        );
    }

    // ---- Task 1.5: observability -----------------------------------------

    #[test]
    fn turn_summary_payload_schema() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("tests/test_a.py"), "").unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        ledger.record_repo_edit_event(
            &ctx(dir.path(), &scope),
            "tests/test_a.py".to_string(),
            ArtifactRole::Test,
            true,
        );
        let s = ledger.turn_summary();
        assert_eq!(s.event_count, 1);
        assert_eq!(s.dropped_count, 0);
        assert!(!s.overflowed);
        // emit must not panic with a populated ledger
        ledger.emit_turn_summary();
    }

    #[test]
    fn stable_path_hash_is_deterministic_and_hides_raw_path() {
        let a = stable_path_hash("tests/test_a.py");
        let b = stable_path_hash("tests/test_a.py");
        assert_eq!(a, b, "hash must be deterministic for the same input");
        let c = stable_path_hash("tests/test_b.py");
        assert_ne!(a, c, "different inputs must produce different hashes");
        // 16 hex chars (u64 hex)
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    // ---- Task 1.6: grep guards -------------------------------------------

    #[test]
    fn dr3001_not_reexported_from_loop_run_rs() {
        let src = std::fs::read_to_string("src/agent/loop_run.rs").expect("read loop_run.rs");
        assert!(
            !src.contains("pub use artifact_ledger"),
            "DR3-001: artifact_ledger must not be re-exported from loop_run.rs"
        );
        assert!(
            !src.contains("pub(crate) use artifact_ledger"),
            "DR3-001: artifact_ledger must not be pub(crate) re-exported from loop_run.rs"
        );
    }

    #[test]
    fn policy_free_no_completion_decision_import() {
        let src =
            std::fs::read_to_string("src/agent/loop_run/artifact_ledger.rs").expect("read self");
        // Inspect only top-level `use` statements (until the first blank-line
        // boundary after the imports) so the grep guard does not match its
        // own assertion strings further down in this test module. We strip
        // out the `#[cfg(test)] mod tests { ... }` block in advance.
        let head = src
            .split_once("#[cfg(test)]")
            .map(|(head, _)| head)
            .unwrap_or(src.as_str());
        // The forbidden tokens are split-built so they themselves do not
        // appear as a single literal in this file (self-match guard).
        let forbidden_segments: &[(&str, &str)] = &[
            ("super::task_contract::", "CompletionDecision"),
            ("super::task_contract::", "ArtifactRecoveryAction"),
            ("super::repair_job::", "RepairJob"),
            ("super::auto_test::", "VerifierCommand"),
            ("super::verifier_skill::", "EffectiveToolPolicy"),
        ];
        for (prefix, tail) in forbidden_segments {
            let needle = format!("use {prefix}{tail}");
            assert!(
                !head.contains(&needle),
                "policy-free invariant violated: {needle}"
            );
        }
    }

    #[test]
    fn session_snapshot_does_not_contain_artifact_ledger() {
        let src = std::fs::read_to_string("src/session/store.rs").expect("read store.rs");
        // The SessionSnapshot struct must not mention artifact_ledger as a
        // field. We do a coarse check: no `artifact_ledger` token anywhere
        // in store.rs (which would be a field if it existed).
        assert!(
            !src.contains("artifact_ledger"),
            "ledger must not leak into SessionSnapshot (per-turn lifetime)"
        );
    }

    #[test]
    fn no_unsafe_in_artifact_ledger_rs() {
        let src =
            std::fs::read_to_string("src/agent/loop_run/artifact_ledger.rs").expect("read self");
        // Reject Rust-syntactic unsafe blocks / functions / traits / impls.
        // We split the keyword across literals so this assertion itself
        // does not match — only actual code constructs can trigger it.
        let kw = "un".to_string() + "safe";
        let needles = [
            format!("{kw} {{"),
            format!("{kw} fn "),
            format!("{kw} trait "),
            format!("{kw} impl "),
            format!("{kw} extern "),
        ];
        for needle in &needles {
            assert!(
                !src.contains(needle),
                "artifact_ledger.rs must not contain {needle:?}"
            );
        }
    }

    // ArtifactState rows with `path: None` must not be admittable as ledger
    // events (Issue #659 §3 / DR2-002).
    #[test]
    fn empty_path_string_is_rejected_at_admission() {
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        let err = admit_event(
            &ctx(dir.path(), &scope),
            super::super::artifact_ownership::NestedTestAdmission::default(),
            "",
            ArtifactRole::Test,
            AdmissionOriginInputs::RepoEdit {
                edited_this_turn: true,
            },
        )
        .unwrap_err();
        assert_eq!(err, AdmissionRejection::PathMissing);
    }

    #[test]
    fn explicit_scope_admit_preserves_path_for_subtree() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("0517_003/app")).unwrap();
        std::fs::write(dir.path().join("0517_003/app/main.py"), "").unwrap();
        let scope = TaskWorkspaceScope {
            mode: ScopeMode::Explicit {
                paths: vec![PathBuf::from("0517_003")],
            },
        };
        let mut ledger = ArtifactLedger::new();
        let res = ledger.record_existing_event(
            &ctx(dir.path(), &scope),
            "0517_003/app/main.py".to_string(),
            ArtifactRole::Implementation,
        );
        let ev = res.expect("explicit scope subtree path should admit");
        assert_eq!(ev.path, "0517_003/app/main.py");
        assert_eq!(ev.ownership, ArtifactOwnership::Owned);
    }

    // ---- Codex review CB-001..CB-003 regression suite --------------------
    //
    // These tests pin the post-review hardening so a future refactor cannot
    // silently regress the admission / projection contract.

    /// CB-001 regression: a missing leaf under a symlinked ancestor that
    /// escapes the workspace must be rejected at admission. The previous
    /// `classify_ownership`-only gate accepted missing leaves because
    /// `canonical_escape` short-circuits to `false` for non-existent paths.
    #[cfg(unix)]
    #[test]
    fn admit_rejects_missing_leaf_under_symlinked_ancestor_outside_root() {
        use std::os::unix::fs::symlink;
        let outside = tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("escape")).unwrap();
        let work = tempdir().unwrap();
        // `work_root/tests -> /outside/escape`. The leaf `tests/new.py`
        // does not exist yet so `classify_ownership` alone would admit it.
        symlink(outside.path().join("escape"), work.path().join("tests")).unwrap();
        let scope = single_root_scope();
        let err = admit_event(
            &ctx(work.path(), &scope),
            super::super::artifact_ownership::NestedTestAdmission::default(),
            "tests/new.py",
            ArtifactRole::Test,
            AdmissionOriginInputs::RepoEdit {
                edited_this_turn: true,
            },
        )
        .unwrap_err();
        assert_eq!(
            err,
            AdmissionRejection::OutOfScope,
            "missing leaf under symlinked ancestor that escapes work_root must be rejected"
        );
    }

    /// CB-001 regression: a real (in-root) missing leaf under a genuine
    /// directory is still admitted — the ancestor check must not over-
    /// reject. This is the legitimate "scaffold-pending file" path.
    #[test]
    fn admit_accepts_missing_leaf_under_real_in_root_parent() {
        let work = tempdir().unwrap();
        std::fs::create_dir_all(work.path().join("tests")).unwrap();
        let scope = single_root_scope();
        let acc = admit_event(
            &ctx(work.path(), &scope),
            super::super::artifact_ownership::NestedTestAdmission::default(),
            "tests/new_test.py",
            ArtifactRole::Test,
            AdmissionOriginInputs::RepoEdit {
                edited_this_turn: true,
            },
        )
        .expect("missing leaf under real in-root parent must admit");
        assert_eq!(acc.path, "tests/new_test.py");
    }

    /// CB-001 regression: dangling symlink leaf — `symlink_metadata` ok
    /// but `canonicalize` fails — must be rejected. Without the
    /// ancestor-aware check a subsequent Write/Edit would dereference the
    /// link and escape the workspace.
    #[cfg(unix)]
    #[test]
    fn admit_rejects_dangling_symlink_leaf() {
        use std::os::unix::fs::symlink;
        let work = tempdir().unwrap();
        std::fs::create_dir_all(work.path().join("tests")).unwrap();
        // tests/test_a.py -> /outside/does-not-exist
        symlink(
            "/outside/does-not-exist",
            work.path().join("tests/test_a.py"),
        )
        .unwrap();
        let scope = single_root_scope();
        let err = admit_event(
            &ctx(work.path(), &scope),
            super::super::artifact_ownership::NestedTestAdmission::default(),
            "tests/test_a.py",
            ArtifactRole::Test,
            AdmissionOriginInputs::RepoEdit {
                edited_this_turn: true,
            },
        )
        .unwrap_err();
        assert_eq!(err, AdmissionRejection::OutOfScope);
    }

    /// CB-001 regression: `record_verifier_observation` shares the
    /// `admit_path_only` pipeline and must also reject missing leaves
    /// under symlinked ancestors.
    #[cfg(unix)]
    #[test]
    fn record_verifier_observation_rejects_symlinked_ancestor_escape() {
        use std::os::unix::fs::symlink;
        let outside = tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("escape")).unwrap();
        let work = tempdir().unwrap();
        symlink(outside.path().join("escape"), work.path().join("tests")).unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        let accepted = ledger.record_verifier_observation(
            &ctx(work.path(), &scope),
            "tests/new_test.py",
            VerifierObservation {
                argv_path_matched: true,
                last_outcome: VerifierOutcome::Pass,
            },
        );
        assert!(
            !accepted,
            "verifier observation for a missing leaf under a symlinked ancestor that escapes \
             work_root must be rejected"
        );
        assert!(
            ledger
                .verifier_observation_for("tests/new_test.py")
                .is_none()
        );
    }

    /// CB-002 regression: once `overflowed=true`, `owned_test_artifacts`
    /// returns an empty list so the verifier-binding callsite cannot mark
    /// the turn as `Runnable` from a partial ledger view.
    #[test]
    fn projection_owned_test_artifacts_fail_closed_on_overflow() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        // Fill up to the cap with distinct test paths (RepoEdit avoids
        // baseline idempotency dedup).
        for i in 0..MAX_ARTIFACT_LEDGER_EVENTS {
            let p = format!("tests/test_{}.py", i);
            std::fs::write(dir.path().join(&p), "").ok();
            ledger.record_repo_edit_event(&ctx(dir.path(), &scope), p, ArtifactRole::Test, true);
        }
        assert!(!ledger.overflowed());
        // Pre-overflow projection has real entries.
        let pre = ledger.owned_test_artifacts(ArtifactRole::Test);
        assert!(!pre.is_empty());
        // One more event overflows the cap.
        let extra = "tests/test_overflow.py".to_string();
        std::fs::write(dir.path().join(&extra), "").ok();
        let _ = ledger.record_repo_edit_event(
            &ctx(dir.path(), &scope),
            extra,
            ArtifactRole::Test,
            true,
        );
        assert!(ledger.overflowed());
        assert_eq!(
            ledger.owned_test_artifacts(ArtifactRole::Test),
            Vec::<String>::new(),
            "overflowed ledger must return empty owned_test_artifacts (CB-002 fail-closed)"
        );
    }

    /// CB-002 regression: once `overflowed=true`,
    /// `required_artifacts_completed` returns `false` for every role so
    /// completion cannot be claimed from a partial ledger view.
    #[test]
    fn projection_required_artifacts_completed_fail_closed_on_overflow() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        for i in 0..MAX_ARTIFACT_LEDGER_EVENTS {
            let p = format!("tests/test_{}.py", i);
            std::fs::write(dir.path().join(&p), "").ok();
            ledger.record_repo_edit_event(&ctx(dir.path(), &scope), p, ArtifactRole::Test, true);
        }
        let extra = "tests/test_overflow.py".to_string();
        std::fs::write(dir.path().join(&extra), "").ok();
        let _ = ledger.record_repo_edit_event(
            &ctx(dir.path(), &scope),
            extra,
            ArtifactRole::Test,
            true,
        );
        assert!(ledger.overflowed());
        let contract = test_contract(vec![ArtifactRole::Test, ArtifactRole::Implementation], true);
        let completed = ledger.required_artifacts_completed(&contract);
        assert_eq!(completed.get(&ArtifactRole::Test).copied(), Some(false));
        assert_eq!(
            completed.get(&ArtifactRole::Implementation).copied(),
            Some(false)
        );
    }

    /// CB-002 regression: once `overflowed=true`, `active_job_candidates`
    /// returns an empty list so Issue #660 arbitration cannot promote a
    /// role from a partial ledger view.
    #[test]
    fn projection_active_job_candidates_fail_closed_on_overflow() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        for i in 0..MAX_ARTIFACT_LEDGER_EVENTS {
            let p = format!("tests/test_{}.py", i);
            std::fs::write(dir.path().join(&p), "").ok();
            ledger.record_repo_edit_event(&ctx(dir.path(), &scope), p, ArtifactRole::Test, true);
        }
        let extra = "tests/test_overflow.py".to_string();
        std::fs::write(dir.path().join(&extra), "").ok();
        let _ = ledger.record_repo_edit_event(
            &ctx(dir.path(), &scope),
            extra,
            ArtifactRole::Test,
            true,
        );
        let contract = test_contract(vec![ArtifactRole::Test, ArtifactRole::Implementation], true);
        assert_eq!(
            ledger.active_job_candidates(&contract),
            Vec::<ArtifactRole>::new(),
            "overflowed ledger must return empty active_job_candidates (CB-002 fail-closed)"
        );
    }

    /// CB-003 regression: `record_verifier_observation` caps the
    /// secondary index at `MAX_ARTIFACT_LEDGER_OBSERVATIONS`. New path
    /// insertions past the cap are rejected, `dropped_count` /
    /// `overflowed` are updated, and upserts on already-bound paths still
    /// succeed (per-turn latest semantics preserved).
    #[test]
    fn verifier_observation_cap_rejects_new_paths_and_marks_overflow() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        // Saturate the index with distinct paths.
        for i in 0..MAX_ARTIFACT_LEDGER_OBSERVATIONS {
            let p = format!("tests/test_obs_{}.py", i);
            std::fs::write(dir.path().join(&p), "").ok();
            assert!(ledger.record_verifier_observation(
                &ctx(dir.path(), &scope),
                &p,
                VerifierObservation {
                    argv_path_matched: true,
                    last_outcome: VerifierOutcome::Pass,
                },
            ));
        }
        assert!(!ledger.overflowed());
        // Upsert on an existing path is still allowed.
        assert!(ledger.record_verifier_observation(
            &ctx(dir.path(), &scope),
            "tests/test_obs_0.py",
            VerifierObservation {
                argv_path_matched: true,
                last_outcome: VerifierOutcome::Fail,
            },
        ));
        assert!(!ledger.overflowed());
        // One more distinct path overflows.
        let extra = "tests/test_obs_overflow.py".to_string();
        std::fs::write(dir.path().join(&extra), "").ok();
        let pre_dropped = ledger.dropped_count();
        let accepted = ledger.record_verifier_observation(
            &ctx(dir.path(), &scope),
            &extra,
            VerifierObservation {
                argv_path_matched: true,
                last_outcome: VerifierOutcome::NotRun,
            },
        );
        assert!(!accepted);
        assert!(ledger.overflowed());
        assert_eq!(ledger.dropped_count(), pre_dropped + 1);
        assert!(ledger.verifier_observation_for(&extra).is_none());
    }

    /// CB-003 follow-up: `MAX_ARTIFACT_LEDGER_OBSERVATIONS` must stay in
    /// lockstep with `MAX_ARTIFACT_LEDGER_EVENTS` so the two structures
    /// have symmetric DoS resistance and the design comment is kept
    /// honest.
    #[test]
    fn observation_cap_matches_event_cap() {
        assert_eq!(
            MAX_ARTIFACT_LEDGER_OBSERVATIONS, MAX_ARTIFACT_LEDGER_EVENTS,
            "observation cap and event cap must stay symmetric (CB-003 design contract)"
        );
    }

    // ---- Issue #659 PR-001: log_context schema alignment -----------------
    // Section 7.1 contract:
    //   * `event_recorded` payload carries `turn_index` + `session_id`
    //   * `turn_summary` payload carries `turn_index` + `session_id`
    //   * `divergence_detected` payload carries bounded masked path-hash
    //     lists (max 16, see `MAX_DIVERGENCE_PATH_HASHES`)
    //
    // The first two are exercised here at the module level; the third is
    // exercised end-to-end from `artifact_ledger_phase2_tests` (it lives
    // in `turn.rs`, not in this module).

    #[test]
    fn log_context_defaults_to_empty_then_set_log_context_updates_it() {
        let mut ledger = ArtifactLedger::new();
        let default_ctx = ledger.log_context().clone();
        assert_eq!(default_ctx, ArtifactLedgerLogContext::default());
        ledger.set_log_context(ArtifactLedgerLogContext::new("sess-abc", 7));
        let got = ledger.log_context().clone();
        assert_eq!(got.session_id, "sess-abc");
        assert_eq!(got.turn_index, 7);
    }

    #[test]
    fn clear_preserves_log_context_so_seed_call_order_is_independent() {
        // `turn.rs::clear_per_turn_ledger_state_for_turn` calls `clear()`
        // once per turn and `set_log_context()` separately. The order is
        // documented in `clear()`'s doc comment — verify clear() does NOT
        // wipe the context so callers can stamp it before or after the
        // reset without changing the per-event payload.
        let mut ledger = ArtifactLedger::new();
        ledger.set_log_context(ArtifactLedgerLogContext::new("sess-keep", 9));
        ledger.clear();
        assert_eq!(ledger.log_context().session_id, "sess-keep");
        assert_eq!(ledger.log_context().turn_index, 9);
    }

    #[test]
    fn bounded_masked_path_hashes_caps_at_sixteen_and_masks() {
        // 20 distinct paths -> 16 entries.
        let inputs: Vec<String> = (0..20).map(|i| format!("tests/test_{i}.py")).collect();
        let hashes = bounded_masked_path_hashes(inputs.iter().map(String::as_str));
        assert_eq!(hashes.len(), MAX_DIVERGENCE_PATH_HASHES);
        for h in &hashes {
            assert_eq!(h.len(), 16, "stable_path_hash returns 16-char hex");
            assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn bounded_masked_path_hashes_matches_event_recorded_path_hash_shape() {
        // Hash space MUST match `event_recorded.path_hash` exactly so a
        // dataset consumer can join divergence rows against per-event rows.
        let path = "tests/test_join.py";
        let expected = stable_path_hash(&mask_secrets(path));
        let hashes = bounded_masked_path_hashes([path].iter().copied());
        assert_eq!(hashes, vec![expected]);
    }

    // ---- Issue #663 Phase B: RequiredArtifactsProjection -----------------

    #[test]
    fn required_artifacts_projection_empty_ledger_reports_all_false() {
        let ledger = ArtifactLedger::new();
        let contract = test_contract(vec![ArtifactRole::Implementation, ArtifactRole::Test], true);
        let p = ledger.required_artifacts_completed_projection(&contract);
        assert!(!p.overflowed());
        assert!(!p.is_satisfied(ArtifactRole::Implementation));
        assert!(!p.is_satisfied(ArtifactRole::Test));
    }

    #[test]
    fn required_artifacts_projection_marks_role_with_owned_event() {
        // Issue #663 Phase B Task B.5 — when a role has at least one Owned
        // RepoEdit event in the ledger, the projection's `is_satisfied`
        // for that role MUST be `true`.
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("tests/test_a.py"), "").unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        ledger.record_repo_edit_event(
            &ctx(dir.path(), &scope),
            "tests/test_a.py".to_string(),
            ArtifactRole::Test,
            true,
        );
        let contract = test_contract(vec![ArtifactRole::Implementation, ArtifactRole::Test], true);
        let p = ledger.required_artifacts_completed_projection(&contract);
        assert!(p.is_satisfied(ArtifactRole::Test));
        assert!(!p.is_satisfied(ArtifactRole::Implementation));
        assert!(!p.overflowed());
    }

    #[test]
    fn required_artifacts_projection_fail_closed_on_overflow() {
        // Issue #663 Phase B Task B.5 — when the ledger is overflowed,
        // `is_satisfied` returns `false` for every role (fail-closed)
        // regardless of any recorded events. DR1-002 / DR3-002 / R1.
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("tests/test_a.py"), "").unwrap();
        let scope = single_root_scope();
        let mut ledger = ArtifactLedger::new();
        ledger.record_repo_edit_event(
            &ctx(dir.path(), &scope),
            "tests/test_a.py".to_string(),
            ArtifactRole::Test,
            true,
        );
        // Force overflowed state directly via append loop until the cap
        // is exceeded.
        for i in 0..(MAX_ARTIFACT_LEDGER_EVENTS + 2) {
            let path = format!("tests/test_overflow_{i}.py");
            std::fs::write(dir.path().join(&path), "").ok();
            ledger.record_repo_edit_event(&ctx(dir.path(), &scope), path, ArtifactRole::Test, true);
        }
        assert!(ledger.overflowed(), "fixture: ledger must overflow");
        let contract = test_contract(vec![ArtifactRole::Test], true);
        let p = ledger.required_artifacts_completed_projection(&contract);
        assert!(p.overflowed());
        assert!(
            !p.is_satisfied(ArtifactRole::Test),
            "overflowed projection MUST be fail-closed for every role"
        );
    }
}
