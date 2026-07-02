//! Issue #929 (P1b) — combined 6-kind capability-invariant matrix (in-crate
//! `#[cfg(test)]`).
//!
//! Consolidates the cross-kind capability invariants that are otherwise
//! scattered across `verifier.rs`'s own `mod tests` (`allows_process_exec` /
//! `requires_executable_verifier` / round-trip) and the per-kind capability E2E
//! mods (`is_coding` in `scaffold_coding_guard_e2e_tests.rs`) into ONE canonical,
//! table-driven matrix. The value is discoverability — a single source of truth
//! for "what every `TaskKind` does across every capability axis" — not new
//! coverage (the invariants are already individually asserted elsewhere; the
//! intentional overlap is documentation-as-test and CI catches any drift).
//!
//! Issue #944 extends the matrix to the split trait shape: every kind has an
//! `AcceptancePolicy`, validating kinds stay behind `Verifier`, and only
//! `CodingCapability` is reachable as a `Remediator`.
//!
//! Pattern lifted from `scaffold_coding_guard_e2e_tests.rs` / `data_capability_e2e_tests.rs`
//! (CB-001 / DR3-001 precedent): an in-crate `#[cfg(test)] mod` that drives the
//! production capability spine + verifier registry directly, with no Ollama and
//! no process spawn. The production binary excludes this module, and every seam
//! is reached through the existing `pub(super)` surface — no production
//! visibility change.

use super::task_contract::TaskKind;
// `verifier_for_task_kind` returns `&'static dyn Verifier`; `.task_kind()` is
// dispatched via the trait object's vtable, so the `Verifier` trait does NOT
// need to be imported here (trait-in-scope is only required to call trait
// methods on a *concrete* type, not on a `dyn Trait`).
use super::verifier::{capability_for, remediator_for_task_kind, verifier_for_task_kind};

// ---------------------------------------------------------------------------
// Canonical expectation SSOT.
//
// `expected` is a **wildcard-free exhaustive `match`**: it deliberately has NO
// `_` arm, so adding a seventh `TaskKind` is a compile error *here*, forcing the
// matrix to be extended in exactly one place. This mirrors the OCP / fail-safe
// explicit enumeration in production (`capability_for` / `is_coding` /
// `requires_executable_verifier` / `verifier_for_task_kind`) at the test layer.
// (`TaskKind` has no `all()` totality helper like `ArtifactRole::all()`, so this
// match is what supplies the compile-time forcing — the `ALL_KINDS` array below
// is only an iteration helper and does NOT, on its own, force a 7th-kind
// update.)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Expected {
    /// `AcceptancePolicy::allows_process_exec()` — verifier child-process spawn.
    allows_process_exec: bool,
    /// `AcceptancePolicy::is_coding()` — scaffold / deterministic-fallback gate.
    is_coding: bool,
    /// `AcceptancePolicy::requires_executable_verifier(false)` — the historical
    /// `coding_verifier_required` gate when the task is *not* verifier-free.
    requires_exec_verifier_when_not_free: bool,
    /// `remediator_for_task_kind(kind).is_some()` — coding-only semantic repair.
    has_remediator: bool,
}

fn expected(kind: TaskKind) -> Expected {
    match kind {
        TaskKind::Coding => Expected {
            allows_process_exec: true,
            is_coding: true,
            requires_exec_verifier_when_not_free: true,
            has_remediator: true,
        },
        TaskKind::Docs
        | TaskKind::Data
        | TaskKind::Research
        | TaskKind::Ops
        | TaskKind::Authoring => Expected {
            allows_process_exec: false,
            is_coding: false,
            requires_exec_verifier_when_not_free: false,
            has_remediator: false,
        },
        // NO `_` arm — a 7th `TaskKind` must be added explicitly above.
    }
}

/// Iteration helper over every `TaskKind`. **Manually maintained** — when a new
/// `TaskKind` is added, append it here (the wildcard-free `expected` match
/// compile-forces the expected *value*, but cannot force this list; see
/// `all_kinds_is_unique_and_consistent_with_expected` for the limits).
const ALL_KINDS: [TaskKind; 6] = [
    TaskKind::Coding,
    TaskKind::Docs,
    TaskKind::Data,
    TaskKind::Research,
    TaskKind::Ops,
    TaskKind::Authoring,
];

// ---------------------------------------------------------------------------
// Invariant 1 — `allows_process_exec()` is true iff Coding.
// ---------------------------------------------------------------------------
#[test]
fn allows_process_exec_matches_matrix() {
    for kind in ALL_KINDS {
        assert_eq!(
            capability_for(kind).kind(),
            kind,
            "capability_for round-trip mismatch for {kind:?}"
        );
        assert_eq!(
            capability_for(kind).allows_process_exec(),
            expected(kind).allows_process_exec,
            "allows_process_exec mismatch for {kind:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Invariant 2 — `requires_executable_verifier()` is Coding-only and respects
// doc-suppression: for every kind the `true` (verifier-free) argument yields
// `false`, and only Coding requires an executable verifier when not verifier-free
// (§5.1: a non-coding task can never be lifted into executable verification).
// ---------------------------------------------------------------------------
#[test]
fn requires_executable_verifier_matches_matrix() {
    for kind in ALL_KINDS {
        assert_eq!(
            capability_for(kind).requires_executable_verifier(false),
            expected(kind).requires_exec_verifier_when_not_free,
            "requires_executable_verifier(false) mismatch for {kind:?}"
        );
        assert!(
            !capability_for(kind).requires_executable_verifier(true),
            "verifier-free document task must never require an executable verifier ({kind:?})"
        );
    }
}

// ---------------------------------------------------------------------------
// Invariant 3 — `is_coding()` is true iff Coding.
// ---------------------------------------------------------------------------
#[test]
fn is_coding_matches_matrix() {
    for kind in ALL_KINDS {
        assert_eq!(
            capability_for(kind).is_coding(),
            expected(kind).is_coding,
            "is_coding mismatch for {kind:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Invariant 4 — verifier registry round-trip: `verifier_for_task_kind(k)`
// selects an adapter whose `task_kind()` is `k` (the #918 OCP fail-safe; e.g.
// `AuthoringVerifier` must not reuse `&DOCS_VERIFIER`, which would return Docs).
// ---------------------------------------------------------------------------
#[test]
fn verifier_for_task_kind_round_trip() {
    for kind in ALL_KINDS {
        assert_eq!(
            verifier_for_task_kind(kind).task_kind(),
            kind,
            "verifier_for_task_kind round-trip mismatch for {kind:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Invariant 5 — Remediator is Coding-only. Non-coding capability paths must not
// grow a trivial or accidental remediation adapter.
// ---------------------------------------------------------------------------
#[test]
fn remediator_registry_is_coding_only() {
    for kind in ALL_KINDS {
        let remediator = remediator_for_task_kind(kind);
        assert_eq!(
            remediator.is_some(),
            expected(kind).has_remediator,
            "remediator availability mismatch for {kind:?}"
        );
        if let Some(remediator) = remediator {
            assert_eq!(
                remediator.task_kind(),
                kind,
                "remediator round-trip mismatch for {kind:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// ALL_KINDS internal sanity (no duplicates / each routes through the SSOT).
//
// NOTE on the limits of this guard (CB-001): the compile-time "a new TaskKind
// must be handled" forcing lives in the wildcard-free `expected()` match (and in
// the production explicit-enumeration matches). `ALL_KINDS` is, unavoidably, a
// MANUALLY maintained iteration list — `TaskKind` has no `all()` totality helper
// (cf. `ArtifactRole::all()`), and there is no purely test-only construct that
// can compile-force its completeness without a production change (descoped by
// DD3). So a 7th kind added to `expected()` but forgotten here would be silently
// skipped by the loops above. This test does NOT claim to close that gap; it
// only guards the list's own consistency (no dup / no stale entry / every listed
// kind has an `expected()` arm). When adding a kind, append it here too.
// ---------------------------------------------------------------------------
#[test]
fn all_kinds_is_unique_and_consistent_with_expected() {
    let mut seen: Vec<TaskKind> = Vec::new();
    for kind in ALL_KINDS {
        assert!(
            !seen.contains(&kind),
            "ALL_KINDS contains a duplicate: {kind:?}"
        );
        seen.push(kind);
        // Every listed kind must have a canonical expectation (no panic / arm
        // exists) — keeps ALL_KINDS from carrying a stale/invalid entry.
        let _ = expected(kind);
    }
}

#[test]
fn coding_is_coding_and_allows_process_exec_stay_coincident() {
    // `allows_process_exec` and `is_coding` share today's truth table but are
    // independent decision axes (verifier spawn vs scaffold materialize,
    // `verifier.rs:260-265`). Pin the current coincidence for Coding so an
    // accidental divergence is caught.
    assert_eq!(
        capability_for(TaskKind::Coding).is_coding(),
        capability_for(TaskKind::Coding).allows_process_exec(),
        "Coding's is_coding and allows_process_exec must stay coincident"
    );
}
