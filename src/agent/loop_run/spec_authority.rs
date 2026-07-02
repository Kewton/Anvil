//! Issue #647 (Phase A.2): spec-authority enum + tie-break scoring +
//! deterministic test/impl weakening detectors.
//!
//! Visibility: every export here is `pub(super)` and **must not** be
//! re-exported from `src/agent/loop_run.rs` (CLAUDE.md DR3-001). Future
//! consumers (`super::repair_job` for `SemanticRepairPlan` and `super::turn`
//! for the repair-editor admission boundary) reach in via sibling import.
//!
//! # Why an enum + scoring (design judgment #4)
//!
//! `SpecAuthority` represents *which single source of truth wins* at a
//! decision point. The "three-artifact consensus" case is **not** an
//! authority — it is a tie-break signal. Modelling it as a scoring layer
//! (`ArtifactConsensus`) keeps the enum small (5 closed variants) and
//! testable as pure functions.
//!
//! # `RepairRole` alias (DR1-009)
//!
//! `RepairRole` is a type alias for `task_contract::ArtifactRole`. The alias
//! is used by **new semantic-repair callsites** in this and downstream
//! modules so the reader knows "this `ArtifactRole` participates in repair
//! planning". Existing modules (`task_contract.rs`, `artifact_ownership.rs`,
//! `completion_evidence.rs`) keep the `ArtifactRole` name.
//!
//! # Weakening detectors (DR1-007 / S1-012)
//!
//! Each of the 9 patterns is implemented as an **independent pure function**
//! (OCP: adding a new pattern is one new pure fn + one orchestration line —
//! the existing detectors stay untouched). Detection is line-based string
//! comparison; no framework-specific literals are hard-coded.
//!
//! # Phase A.2 dead-code policy
//!
//! Phase A.2 lands the enum + scoring + detectors ahead of their consumers
//! (Phase D wires `select_authority` into the diagnostic pipeline; Phase F
//! wires the weakening detectors into the repair-editor admission boundary).
//! Until those Phases land, the `pub(super)` surface is exercised only by
//! in-module unit tests, so we silence `dead_code` at the module root rather
//! than dropping the API.

#![allow(dead_code)]

use super::repair_job::sanitize_repair_job_text_with_char_cap;
use super::task_contract::ArtifactRole;

/// Maximum char cap applied to user-supplied / LLM-supplied free-form
/// `reason` text on `ArtifactConsensus` (DR4-004 mirror of the
/// `semantic_failure.rs` cap).
const MAX_CONSENSUS_REASON_CHARS: usize = 240;

/// Ranked spec-authority levels. `PartialOrd` / `Ord` follow variant order,
/// so `UserRequest < BehaviorContract < ... < LlmGeneratedTest` — **smaller
/// is higher authority**.
///
/// `VerifiedPublicInterface` is a forward-extensibility placeholder
/// (DR1-005): `select_authority` does not return it today. The variant exists
/// so the API surface is stable when a later Issue wires the
/// session/turn-local cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum SpecAuthority {
    /// Highest authority — explicit human request.
    UserRequest,
    /// Derived deterministic behavior contract (`RequiredBehaviorContract`).
    BehaviorContract,
    /// Placeholder for "this interface already passed a verifier in scope"
    /// (DR1-005 / S5-005). **Dead variant** in this Issue — `select_authority`
    /// will not return it even if a caller passes it in.
    VerifiedPublicInterface,
    /// Type / schema / validation invariants in the implementation.
    ImplementationContract,
    /// Lowest authority — LLM-generated test assertions.
    LlmGeneratedTest,
}

/// Type alias for `ArtifactRole` used by **new** semantic-repair-flavored
/// callsites. The two names refer to the exact same type — no conversion
/// is needed at the boundary (DR1-009).
#[allow(dead_code)]
pub(super) type RepairRole = ArtifactRole;

/// Pairwise consensus signal among the three artifact roles
/// (Implementation / Test / UsageDocs). When two agree and one disagrees,
/// the agreeing side becomes the tie-break winner.
///
/// `reason` is sanitized at construction via
/// [`ArtifactConsensus::new`] (DR4-004).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ArtifactConsensus {
    pub(super) agreeing: Vec<ArtifactRole>,
    pub(super) dissenting: Vec<ArtifactRole>,
    pub(super) reason: String,
}

impl ArtifactConsensus {
    /// Construct an `ArtifactConsensus` with the `reason` text sanitized to
    /// `MAX_CONSENSUS_REASON_CHARS` chars via the SSOT entry
    /// (`sanitize_repair_job_text_with_char_cap`, DR1-001 / DR4-004).
    #[allow(dead_code)] // consumer arrives in Phase D
    pub(super) fn new(
        agreeing: Vec<ArtifactRole>,
        dissenting: Vec<ArtifactRole>,
        raw_reason: &str,
    ) -> Self {
        Self {
            agreeing,
            dissenting,
            reason: sanitize_repair_job_text_with_char_cap(raw_reason, MAX_CONSENSUS_REASON_CHARS),
        }
    }
}

/// Pick the highest-authority candidate, falling back to `consensus` only
/// when (a) the candidate list yielded multiple *distinct* top-tier entries
/// (genuine tie) or (b) the candidate list is empty.
///
/// `VerifiedPublicInterface` is filtered out before scoring (DR1-005) so it
/// never wins the selection in this Issue.
///
/// CB-001 (Codex review): duplicate same-variant candidates are **not** a tie.
/// Since `SpecAuthority` has a strict total order, the only way `top_count > 1`
/// could trigger the consensus branch previously was when the same variant
/// appeared multiple times — that should resolve to that variant, not be
/// demoted to a weaker authority via consensus. We dedup before tie-checking,
/// and we additionally guard consensus output from demoting the top: the
/// consensus winner is only adopted if it is at least as strong as `top`.
#[allow(dead_code)] // consumer arrives in Phase D
pub(super) fn select_authority(
    candidates: &[SpecAuthority],
    consensus: Option<&ArtifactConsensus>,
) -> Option<SpecAuthority> {
    // DR1-005 / S5-005: VerifiedPublicInterface is a dead variant in this Issue.
    let mut filtered: Vec<SpecAuthority> = candidates
        .iter()
        .copied()
        .filter(|c| *c != SpecAuthority::VerifiedPublicInterface)
        .collect();

    if filtered.is_empty() {
        // No usable candidates → fall back to consensus if available.
        return consensus_to_authority(consensus);
    }

    filtered.sort();
    let top = filtered[0];

    // CB-001: dedup duplicates of the *same* variant — these are not a tie.
    // After dedup, a "tie" means two *distinct* variants share the top rank
    // (impossible today because variant order is total, but we keep the
    // dedup as defence-in-depth for future variant additions).
    let mut distinct = filtered.clone();
    distinct.dedup();
    let top_count = distinct.iter().filter(|c| **c == top).count();
    if top_count > 1
        && let Some(c) = consensus_to_authority(consensus)
        && c <= top
    {
        // Adopt consensus only when it does NOT demote the top authority
        // (smaller variant index = higher authority).
        return Some(c);
    }
    Some(top)
}

/// Issue #647 (SF1 V3): minimal cross-turn history hint that the
/// `SpecAuthorityInput` builder can read to decide whether
/// `has_verified_public_interface` should fire.
///
/// Kept deliberately small (one bool) — wider session-store history is out
/// of scope for SF1 V3 (S5-005 dead-variant policy still applies on the
/// resolver side). When future Issues plumb richer history, this struct is
/// the only growth point for the builder signature.
///
/// `Default::default()` is the safe production fallback when no caller
/// context is available (e.g. test wrappers or fresh sessions).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct AgentHistoryHint {
    /// True iff the agent already observed a verifier success for the
    /// active task *within this run-actor-loop iteration*. Mirrors the
    /// local `task_contract_verifier_passed_in_loop` flag in
    /// `Agent::handle_user_message`.
    pub(super) verifier_passed_in_loop: bool,
}

/// Issue #647 (SF1 V3.1): explicit-spec keyword set used by
/// [`detect_explicit_spec_in_user_request`]. Ascii + Japanese kept in one
/// table so the detector is a pure-string match. Each entry encodes a
/// determinate, prescriptive expression — vague guidance (e.g. "please",
/// "could you") is intentionally excluded.
const EXPLICIT_SPEC_KEYWORDS: &[&str] = &[
    // Ascii: prescriptive modal verbs and contract phrasing.
    "must ",
    "must return",
    "must accept",
    "must raise",
    "must not",
    "should ",
    "should return",
    "should accept",
    "should not",
    "shall ",
    "shall return",
    "specification",
    "spec:",
    "expects exactly",
    "expects:",
    "api contract",
    "contract:",
    // Japanese: prescriptive phrasing.
    "仕様",
    "必須",
    "必ず",
    "返却すること",
    "返すこと",
    "返さなければならない",
    "してはならない",
    "とすること",
];

/// Minimum count of distinct explicit-spec keywords that must appear in
/// `active_request` for the detector to declare a "user-request match".
///
/// SF1 V3 design judgment: a single occurrence of "should" is too noisy
/// (it often appears in conversational filler). Two distinct hits raise
/// the bar high enough that the request is plausibly *prescriptive*
/// without forcing the user to write a formal grammar.
const EXPLICIT_SPEC_MIN_DISTINCT_HITS: usize = 2;

/// Strong section markers that mean the user intentionally introduced a
/// specification block. Unlike conversational words such as "should", one of
/// these markers is enough to elect user-request authority.
const EXPLICIT_SPEC_STRONG_MARKERS: &[&str] = &[
    "contract:",
    "spec:",
    "specification:",
    "requirements:",
    "要件:",
    "仕様:",
    "仕様は",
];

/// Issue #647 (SF1 V3.1): cheap heuristic that classifies an
/// `active_request` as "carries an explicit specification" iff at least
/// `EXPLICIT_SPEC_MIN_DISTINCT_HITS` distinct, **non-overlapping** keyword
/// spans from `EXPLICIT_SPEC_KEYWORDS` appear in the request.
///
/// Pure function — no I/O, no LLM dependency. Matching is case-insensitive
/// on the ascii side; the Japanese keywords are matched verbatim (Japanese
/// has no case fold to apply).
///
/// CB-010 (Issue #647 V3 iteration-4): the keyword table intentionally
/// contains overlapping prefixes ("must " vs. "must return", "should "
/// vs. "should return", …) so a single phrase like "must return 404" used
/// to be counted twice and crossed the 2-hit threshold on its own — a
/// false positive. We now collect every match position, sort by start,
/// and greedily drop spans whose `[start, end)` overlaps the previously
/// accepted span. The resulting count is the number of **distinct
/// linguistic occurrences**, not the number of matching keywords.
pub(super) fn detect_explicit_spec_in_user_request(active_request: &str) -> bool {
    if active_request.trim().is_empty() {
        return false;
    }
    let lowered = active_request.to_ascii_lowercase();

    if EXPLICIT_SPEC_STRONG_MARKERS
        .iter()
        .any(|marker| lowered.contains(marker) || active_request.contains(marker))
    {
        return true;
    }

    // CB-010: gather every keyword match as a byte-span and dedup
    // overlapping spans so a single phrase only counts once.
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for keyword in EXPLICIT_SPEC_KEYWORDS {
        let kw_len = keyword.len();
        if kw_len == 0 {
            continue;
        }
        // Walk non-overlapping occurrences of this single keyword.
        // We advance by `kw_len` (safe char-boundary for ascii + multibyte
        // because keywords are stored as whole UTF-8 sequences). Cross-
        // keyword overlap is still caught by the span-dedup step below.
        let mut search_from = 0;
        while let Some(rel) = lowered[search_from..].find(keyword) {
            let start = search_from + rel;
            let end = start + kw_len;
            spans.push((start, end));
            search_from = end;
        }
    }

    if spans.len() < EXPLICIT_SPEC_MIN_DISTINCT_HITS {
        return false;
    }

    // Greedy non-overlapping span count: sort by start, accept a span
    // only if its start is at or beyond the previously accepted end.
    spans.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
    let mut deduped_hits = 0usize;
    let mut last_end = 0usize;
    for (start, end) in spans {
        if start >= last_end {
            deduped_hits += 1;
            last_end = end;
            if deduped_hits >= EXPLICIT_SPEC_MIN_DISTINCT_HITS {
                return true;
            }
        }
    }
    false
}

/// Issue #647 (SF1 V3.3): consensus heuristic computed from the
/// `ContractConflict` fields of a `SemanticFailureReport`. Returns
/// `Some(ArtifactConsensus { agreeing, dissenting, reason })` when exactly
/// two of the three roles agree (after normalization) and the third
/// disagrees; otherwise returns `None` (covers both "all three agree" and
/// "all three differ").
///
/// Normalization is intentionally cheap: lower-case + collapsed-whitespace
/// trim. The Issue spec explicitly forbids NLP — this stays a pure string
/// match.
pub(super) fn detect_consensus_from_contract_conflict(
    implementation: &str,
    test: &str,
    usage_docs: &str,
) -> Option<ArtifactConsensus> {
    let impl_norm = normalize_consensus_text(implementation);
    let test_norm = normalize_consensus_text(test);
    let docs_norm = normalize_consensus_text(usage_docs);
    if impl_norm.is_empty() || test_norm.is_empty() || docs_norm.is_empty() {
        return None;
    }
    let impl_test = impl_norm == test_norm;
    let impl_docs = impl_norm == docs_norm;
    let test_docs = test_norm == docs_norm;
    // Exactly two roles agree → tie-break consensus.
    match (impl_test, impl_docs, test_docs) {
        (true, false, false) => Some(ArtifactConsensus::new(
            vec![ArtifactRole::Implementation, ArtifactRole::Test],
            vec![ArtifactRole::UsageDocs],
            "impl and test agree; usage_docs dissents",
        )),
        (false, true, false) => Some(ArtifactConsensus::new(
            vec![ArtifactRole::Implementation, ArtifactRole::UsageDocs],
            vec![ArtifactRole::Test],
            "impl and usage_docs agree; test dissents",
        )),
        (false, false, true) => Some(ArtifactConsensus::new(
            vec![ArtifactRole::Test, ArtifactRole::UsageDocs],
            vec![ArtifactRole::Implementation],
            "test and usage_docs agree; impl dissents",
        )),
        // All three agree, or all three differ → no usable tie-break.
        _ => None,
    }
}

/// Normalize a `ContractConflict` field for consensus comparison.
///
/// Lower-case + collapse all whitespace runs to a single space + trim.
fn normalize_consensus_text(text: &str) -> String {
    let lowered = text.to_ascii_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut last_was_ws = true; // suppress leading whitespace
    for ch in lowered.chars() {
        if ch.is_whitespace() {
            if !last_was_ws {
                out.push(' ');
                last_was_ws = true;
            }
        } else {
            out.push(ch);
            last_was_ws = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Issue #647 (SF1 V3.2 + CB-011): "verified public interface" detector.
///
/// **Dead variant placeholder until artifact identity is bound** (CB-011).
/// Earlier iterations returned `hint.verifier_passed_in_loop` directly, but
/// the `verifier_passed_in_loop` bit is *turn-local* (S5-005) without being
/// *artifact-specific*: it tells us "some artifact passed the verifier in
/// this actor-loop iteration", not "the artifact implicated by the current
/// `SemanticFailureReport` passed the verifier". The current
/// `SemanticFailureReport` schema does not carry path / interface identifiers
/// in `affected_cases` / `involved_artifacts` / `contract_conflict`, so there
/// is no safe way to prove path overlap. Lifting authority on the bit alone
/// would let an unrelated verifier success elevate `VerifiedPublicInterface`
/// over `ImplementationContract` for a failure that has nothing to do with
/// the verified artifact — a false-positive authority elevation.
///
/// We therefore keep the detector **constant `false`** in this Issue. The
/// `VerifiedPublicInterface` variant remains in the [`SpecAuthority`] enum
/// (DR1-005 forward-extensibility), the [`AgentHistoryHint`] struct keeps
/// `verifier_passed_in_loop` as a placeholder field, and the production
/// resolve path can never reach the variant. A future Issue that plumbs
/// artifact identity (e.g. failing path / interface identifier on the
/// `SemanticFailureReport`) into the hint will be the single growth point:
/// it can add a `passed_artifact_paths: Vec<String>` field on
/// `AgentHistoryHint`, take a `&SemanticFailureReport` here, and flip the
/// return to `true` only when path overlap is provable.
///
/// Pure function — `hint` is currently unused but kept on the signature to
/// preserve the call-site shape (`turn.rs::build_spec_authority_input_for_active_request`)
/// and to make the future-extension growth point obvious.
pub(super) fn detect_verified_public_interface_from_history(_hint: AgentHistoryHint) -> bool {
    // CB-011: hold the line at `false` until artifact identity is bound.
    // See doc comment for the full rationale.
    false
}

/// Issue #647 (SF1): aggregated input passed by `turn.rs` to [`resolve`]
/// when constructing a `SemanticRepairPlan`. Each flag is computed by the
/// caller from a different source (the user request, the
/// `RequiredBehaviorContract`, the turn-local task state, etc.); collecting
/// them in a single struct lets the resolution rules stay declarative and
/// keeps the production callsite a single line.
///
/// Fields:
/// - `has_user_request_match`: the failing artifact lines up with an
///   *explicit* spec in the user request (detection is a future Issue —
///   today the caller passes `false`).
/// - `has_behavior_contract`: a `RequiredBehaviorContract` has actionable
///   signal in scope (operations / domain_terms / etc.).
/// - `has_verified_public_interface`: a public interface that already
///   passed verification is in scope. **Dead variant** (S5-005); the
///   production caller passes `false`.
/// - `is_newly_generated_task`: this turn produced new implementation /
///   test / README artifacts. When `true` and no `consensus` exists, the
///   resolution returns `LlmGeneratedTest` so test edits get suppressed
///   (test cannot be the lone source of truth on a brand-new task).
/// - `consensus`: optional 2-vs-1 agreement signal across the three
///   artifacts. When present, [`select_authority`] handles tie-break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SpecAuthorityInput {
    pub(super) has_user_request_match: bool,
    pub(super) has_behavior_contract: bool,
    pub(super) has_verified_public_interface: bool,
    pub(super) is_newly_generated_task: bool,
    pub(super) consensus: Option<ArtifactConsensus>,
}

/// Resolve a [`SpecAuthority`] from `input`.
///
/// Decision order (Issue #647 acceptance):
/// 1. `has_user_request_match` → [`SpecAuthority::UserRequest`] (highest).
/// 2. `has_behavior_contract` → [`SpecAuthority::BehaviorContract`].
/// 3. `has_verified_public_interface` →
///    [`SpecAuthority::VerifiedPublicInterface`]. **Dead variant** in this
///    Issue — the production caller passes `false`, so this branch is only
///    reachable from in-module unit tests.
/// 4. `is_newly_generated_task && consensus.is_none()` →
///    [`SpecAuthority::LlmGeneratedTest`] (the *lowest* authority). Issue
///    acceptance treats impl/test/README as equally authoritative on a
///    brand-new task, but if we elected `ImplementationContract` here the
///    repair editor would happily rewrite the test to match buggy impl.
///    Returning the lowest authority instead is the safe default — the
///    weakening detectors get a chance to block test edits before we
///    decide impl is the source of truth.
/// 5. `consensus.is_some()` → apply [`consensus_to_authority_for_resolve`]
///    directly so the agreeing roles drive authority selection (CB-008).
///    The previous implementation delegated to [`select_authority`] with a
///    fixed candidate base `[ImplementationContract, LlmGeneratedTest]`,
///    which always returned `ImplementationContract` by ordered-enum top
///    even when test + usage_docs agreed against impl — the consensus
///    signal became a dead path.
/// 6. Default fallback → [`SpecAuthority::ImplementationContract`].
pub(super) fn resolve(input: &SpecAuthorityInput) -> SpecAuthority {
    if input.has_user_request_match {
        return SpecAuthority::UserRequest;
    }
    if input.has_behavior_contract {
        return SpecAuthority::BehaviorContract;
    }
    if input.has_verified_public_interface {
        return SpecAuthority::VerifiedPublicInterface;
    }
    if input.is_newly_generated_task && input.consensus.is_none() {
        return SpecAuthority::LlmGeneratedTest;
    }
    if let Some(consensus) = input.consensus.as_ref() {
        // CB-008: apply the agreeing-role mapping directly. We deliberately
        // do NOT route through `select_authority` here — its `[Impl,
        // LlmGenTest]` candidate base always returns `Impl` by ordered-enum
        // top, which silently bypassed the consensus signal for the
        // test + usage_docs case.
        return consensus_to_authority_for_resolve(consensus);
    }
    SpecAuthority::ImplementationContract
}

/// Map an `ArtifactConsensus` to the spec authority that the agreeing roles
/// imply. Implementation-agreeing → `ImplementationContract`; otherwise the
/// consensus alone is not enough to elect a higher authority and we return
/// `None`.
fn consensus_to_authority(consensus: Option<&ArtifactConsensus>) -> Option<SpecAuthority> {
    let c = consensus?;
    if c.agreeing.is_empty() {
        return None;
    }
    if c.agreeing.contains(&ArtifactRole::Implementation) {
        Some(SpecAuthority::ImplementationContract)
    } else {
        // Test- or docs-only consensus is weaker than ImplementationContract
        // and isn't enough on its own to win — defer the decision back to the
        // caller, which can fall back to LlmGeneratedTest if nothing else is
        // available.
        Some(SpecAuthority::LlmGeneratedTest)
    }
}

/// CB-008: map an `ArtifactConsensus` to a `SpecAuthority` from `resolve`'s
/// point of view. Unlike [`consensus_to_authority`] (used by
/// [`select_authority`] as a fallback that must not demote an existing
/// candidate top), this mapping is the **primary** decision when no
/// higher-authority signal is available: the agreeing roles drive the
/// outcome, not the ordered-enum top of a fixed candidate base.
///
/// Mapping (closed, deterministic):
/// - agreeing contains `Implementation` → `ImplementationContract`
///   (canonical "stale test" case).
/// - agreeing is `Test` + `UsageDocs` (no `Implementation`) →
///   `BehaviorContract` — the spec-side (test assertions + user-facing
///   docs) agrees against the impl, so we elect the spec-side authority
///   rather than letting the repair editor weaken the test toward buggy
///   impl. This is the CB-008 bug fix's central case.
/// - any other agreeing layout (incl. empty / `Test`-only / `UsageDocs`-only)
///   → `ImplementationContract` as the safe default. A lone Test or lone
///   docs is too thin to elect `BehaviorContract`; the empty case is
///   already filtered out at the caller, but we guard it defensively.
fn consensus_to_authority_for_resolve(consensus: &ArtifactConsensus) -> SpecAuthority {
    let has_impl = consensus.agreeing.contains(&ArtifactRole::Implementation);
    let has_test = consensus.agreeing.contains(&ArtifactRole::Test);
    let has_docs = consensus.agreeing.contains(&ArtifactRole::UsageDocs);

    if has_impl {
        return SpecAuthority::ImplementationContract;
    }
    if has_test && has_docs {
        // Spec-side 2-vs-1: test assertions and user-facing docs agree
        // against the implementation. Elect BehaviorContract so the repair
        // editor cannot weaken the test toward buggy impl.
        return SpecAuthority::BehaviorContract;
    }
    SpecAuthority::ImplementationContract
}

/// Closed list of test/impl weakening patterns detected at the repair
/// editor's admission boundary (S1-004 / S1-007). Each variant is produced
/// by a dedicated pure-function detector (`detect_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum WeakeningPattern {
    // Test patterns (5).
    AssertionDeleted,
    SkipMarkerAdded,
    AssertTrueWeakening,
    TestFunctionDeleted,
    LiteralOnlyExpectedChange,
    // Implementation patterns (4).
    ValidatorDeleted,
    ErrorSwallowed,
    EarlyReturnBypass,
    TypeAnnotationWeakened,
}

/// Orchestration: aggregate every test-side detector for `(before, after)`.
///
/// Each detector is independent (OCP) and the orchestrator is just a flatten
/// over their `Option<WeakeningPattern>` outputs.
#[allow(dead_code)] // consumer arrives in Phase F
pub(super) fn detect_test_weakening(
    relative_path: &str,
    before: &str,
    after: &str,
) -> Vec<WeakeningPattern> {
    [
        detect_assertion_deleted(relative_path, before, after),
        detect_skip_marker_added(relative_path, before, after),
        detect_assert_true_weakening(relative_path, before, after),
        detect_test_function_deleted(relative_path, before, after),
        detect_literal_only_expected_change(relative_path, before, after),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Orchestration: aggregate every impl-side detector for `(before, after)`.
#[allow(dead_code)] // consumer arrives in Phase F
pub(super) fn detect_impl_weakening(
    relative_path: &str,
    before: &str,
    after: &str,
) -> Vec<WeakeningPattern> {
    [
        detect_validator_deleted(relative_path, before, after),
        detect_error_swallowed(relative_path, before, after),
        detect_early_return_bypass(relative_path, before, after),
        detect_type_annotation_weakened(relative_path, before, after),
    ]
    .into_iter()
    .flatten()
    .collect()
}

// ---------- helpers ---------- //

/// Return the set of trimmed non-empty lines in `s` (multiset preserving
/// the first occurrence, not full bag semantics). Used by detectors that
/// reason about "line X exists in before but not in after".
fn line_set(s: &str) -> Vec<String> {
    s.lines().map(|l| l.trim().to_string()).collect()
}

/// Build a multiset (line → count) of trimmed non-empty lines.
fn line_multiset(s: &str) -> std::collections::HashMap<String, isize> {
    let mut m = std::collections::HashMap::new();
    for line in line_set(s) {
        if line.is_empty() {
            continue;
        }
        *m.entry(line).or_insert(0) += 1;
    }
    m
}

/// CB-003: lines whose count in `before` strictly exceeds their count in
/// `after`. The returned `Vec` repeats a line `n` times when `n` extra
/// occurrences were deleted, so detectors that scan it see every deleted
/// occurrence rather than a single deduped representative.
fn lines_only_in_before(before: &str, after: &str) -> Vec<String> {
    let after_counts = line_multiset(after);
    let before_counts = line_multiset(before);
    let mut out: Vec<String> = Vec::new();
    for (line, before_n) in before_counts {
        let after_n = after_counts.get(&line).copied().unwrap_or(0);
        let delta = before_n - after_n;
        if delta > 0 {
            for _ in 0..delta {
                out.push(line.clone());
            }
        }
    }
    out
}

/// CB-003: lines whose count in `after` strictly exceeds their count in
/// `before`. Multiset semantics mirror [`lines_only_in_before`].
fn lines_only_in_after(before: &str, after: &str) -> Vec<String> {
    let before_counts = line_multiset(before);
    let after_counts = line_multiset(after);
    let mut out: Vec<String> = Vec::new();
    for (line, after_n) in after_counts {
        let before_n = before_counts.get(&line).copied().unwrap_or(0);
        let delta = after_n - before_n;
        if delta > 0 {
            for _ in 0..delta {
                out.push(line.clone());
            }
        }
    }
    out
}

// ---------- test-side detectors (5) ---------- //

/// AssertionDeleted: any `assert*`-prefixed line present in `before` but
/// missing in `after` (pure deletion).
fn detect_assertion_deleted(
    _relative_path: &str,
    before: &str,
    after: &str,
) -> Option<WeakeningPattern> {
    let deleted = lines_only_in_before(before, after);
    if deleted.iter().any(|l| l.starts_with("assert")) {
        Some(WeakeningPattern::AssertionDeleted)
    } else {
        None
    }
}

/// SkipMarkerAdded: a skip / ignore / expected-failure marker appears in
/// `after` that wasn't in `before`. Language-level markers only — no
/// framework-specific hardcode beyond the generic marker tokens themselves.
fn detect_skip_marker_added(
    _relative_path: &str,
    before: &str,
    after: &str,
) -> Option<WeakeningPattern> {
    const MARKERS: &[&str] = &[
        "@pytest.mark.skip",
        "@pytest.mark.xfail",
        "#[ignore]",
        "#[should_panic]",
    ];
    let added = lines_only_in_after(before, after);
    if added.iter().any(|l| MARKERS.iter().any(|m| l.contains(m))) {
        Some(WeakeningPattern::SkipMarkerAdded)
    } else {
        None
    }
}

/// AssertTrueWeakening: lines added in `after` that are bare "assert True"
/// / "assert!(true)" / "pass" — common assertion-bypass shapes.
fn detect_assert_true_weakening(
    _relative_path: &str,
    before: &str,
    after: &str,
) -> Option<WeakeningPattern> {
    let before_had_assert = line_set(before).iter().any(|l| l.starts_with("assert"));
    if !before_had_assert {
        return None;
    }
    let added = lines_only_in_after(before, after);
    let matched = added.iter().any(|l| {
        let t = l.trim_end_matches(';').trim();
        t == "assert True" || t == "assert!(true)" || t == "pass" || t == "assert true"
    });
    matched.then_some(WeakeningPattern::AssertTrueWeakening)
}

/// TestFunctionDeleted: a `def test_*` / `fn test_*` start line present in
/// `before` but missing in `after`.
fn detect_test_function_deleted(
    _relative_path: &str,
    before: &str,
    after: &str,
) -> Option<WeakeningPattern> {
    let deleted = lines_only_in_before(before, after);
    let hit = deleted.iter().any(|l| {
        let t = l.trim_start();
        t.starts_with("def test_") || t.starts_with("fn test_")
    });
    hit.then_some(WeakeningPattern::TestFunctionDeleted)
}

/// LiteralOnlyExpectedChange: an `assert`-line in `before` has been replaced
/// by an `assert`-line in `after` where the literal RHS differs, and no
/// other non-assertion lines moved (= test inputs / setup untouched).
fn detect_literal_only_expected_change(
    _relative_path: &str,
    before: &str,
    after: &str,
) -> Option<WeakeningPattern> {
    let before_asserts: Vec<String> = line_set(before)
        .into_iter()
        .filter(|l| l.starts_with("assert"))
        .collect();
    let after_asserts: Vec<String> = line_set(after)
        .into_iter()
        .filter(|l| l.starts_with("assert"))
        .collect();
    let before_non_assert: Vec<String> = line_set(before)
        .into_iter()
        .filter(|l| !l.is_empty() && !l.starts_with("assert"))
        .collect();
    let after_non_assert: Vec<String> = line_set(after)
        .into_iter()
        .filter(|l| !l.is_empty() && !l.starts_with("assert"))
        .collect();

    let asserts_changed = before_asserts != after_asserts && !before_asserts.is_empty();
    let non_assert_unchanged = before_non_assert == after_non_assert;
    (asserts_changed && non_assert_unchanged).then_some(WeakeningPattern::LiteralOnlyExpectedChange)
}

// ---------- impl-side detectors (4) ---------- //

/// ValidatorDeleted: `assert` / `if .* raise` / `?` propagation / bare
/// `raise` / `throw` / `return Err` / `bail!` deleted from implementation
/// code.
///
/// CB-004 (Codex review): a multiline validator
/// ```text
/// if invalid:
///     raise ValueError(...)
/// ```
/// can be silently weakened by deleting only the `raise` line. We therefore
/// treat any standalone validator-terminal line (`raise`, `throw`,
/// `return Err`, `bail!`, `anyhow::bail!`) as a deleted validator, not just
/// the same-line `if ... raise` shape.
fn detect_validator_deleted(
    _relative_path: &str,
    before: &str,
    after: &str,
) -> Option<WeakeningPattern> {
    let deleted = lines_only_in_before(before, after);
    let hit = deleted.iter().any(|l| {
        let t = l.trim_start();
        t.starts_with("assert")
            || (t.starts_with("if ") && l.contains("raise"))
            // `?` propagation lines like `let x = foo()?;` — bare `?` operator.
            || (l.contains("?;") && !l.contains("if "))
            // CB-004: standalone validator-terminal lines (multiline validators).
            || is_validator_terminal_line(t)
    });
    hit.then_some(WeakeningPattern::ValidatorDeleted)
}

/// CB-004: is `t` (already trim_start'd) a standalone validator-terminal
/// statement (`raise ...`, `throw ...`, `return Err(...)`, `bail!(...)`,
/// `anyhow::bail!(...)`)?
fn is_validator_terminal_line(t: &str) -> bool {
    t.starts_with("raise ")
        || t == "raise"
        || t.starts_with("throw ")
        || t.starts_with("throw new ")
        || t.starts_with("return Err(")
        || t == "return Err()"
        || t.starts_with("bail!(")
        || t.starts_with("anyhow::bail!(")
}

/// ErrorSwallowed: new code in `after` that drops error information
/// (`try / except: pass`, `Result::ok()` chain that discards the error,
/// `let _ = x?` is *not* covered — that's still propagation).
fn detect_error_swallowed(
    _relative_path: &str,
    before: &str,
    after: &str,
) -> Option<WeakeningPattern> {
    let added = lines_only_in_after(before, after);

    // Pattern 1: bare `except: pass` style added.
    let bare_except = added.iter().any(|l| {
        let t = l.trim();
        t == "except: pass" || t == "except Exception: pass" || t.starts_with("except:")
    });
    if bare_except {
        return Some(WeakeningPattern::ErrorSwallowed);
    }

    // Pattern 2: `.ok();` (discards the error half of Result) newly added.
    let ok_discard = added.iter().any(|l| {
        let t = l.trim();
        t.ends_with(".ok();") || t.ends_with(".ok().unwrap_or_default();")
    });
    if ok_discard {
        return Some(WeakeningPattern::ErrorSwallowed);
    }

    // Pattern 3: a multiline try block (`try:` then `except: pass` later)
    // can also be detected as a holistic before/after change.
    let after_text = after;
    let before_text = before;
    if after_text.contains("except:")
        && after_text.contains("pass")
        && !(before_text.contains("except:") && before_text.contains("pass"))
    {
        return Some(WeakeningPattern::ErrorSwallowed);
    }

    None
}

/// EarlyReturnBypass: `return` / `return Ok(())` newly added near the top of
/// a function body in `after` (= short-circuits the rest of the body).
fn detect_early_return_bypass(
    _relative_path: &str,
    before: &str,
    after: &str,
) -> Option<WeakeningPattern> {
    let added = lines_only_in_after(before, after);
    let hit = added.iter().any(|l| {
        let t = l.trim();
        t == "return"
            || t == "return;"
            || t == "return None"
            || t == "return None;"
            || t == "return Ok(())"
            || t == "return Ok(());"
            || t == "return True"
            || t == "return True;"
            || t == "return None,"
    });
    hit.then_some(WeakeningPattern::EarlyReturnBypass)
}

/// TypeAnnotationWeakened: structural weakening of type annotations on the
/// **same** identifier (`item_id: int` → `item_id: str`, `x: int` →
/// `x: Any`, `&str` → `&dyn Any`).
///
/// CB-006 (Codex review): the previous string-contains heuristic fired on
/// unrelated lines (e.g. adding `unrelated: Any` to a file that already had
/// `x: int`). We now extract `identifier: annotation` pairs per line and
/// only fire when the same identifier's annotation moved from a
/// strong-typed token to a weak-typed token across `(before, after)`.
fn detect_type_annotation_weakened(
    _relative_path: &str,
    before: &str,
    after: &str,
) -> Option<WeakeningPattern> {
    let before_map = collect_identifier_annotations(before);
    let after_map = collect_identifier_annotations(after);

    for (ident, before_anns) in &before_map {
        let Some(after_anns) = after_map.get(ident) else {
            continue;
        };
        // For each annotation the identifier had in `before`, see if any
        // matching annotation in `after` is a strict weakening of it.
        for ba in before_anns {
            for aa in after_anns {
                if is_annotation_weakening(ba, aa) {
                    return Some(WeakeningPattern::TypeAnnotationWeakened);
                }
            }
        }
    }
    None
}

/// CB-006: collect `identifier -> [annotations]` from every line in `src`.
///
/// Captures the simple shapes covered by Phase A.2:
/// - Python: `ident: TYPE` (parameter / variable / field; TYPE up to `,` `)` `=` end-of-line)
/// - Rust: `ident: TYPE` in `let` / fn params / struct fields (same delimiters)
///
/// More exotic constructs (generic bounds, nested generics with `,`, etc.)
/// are intentionally out-of-scope — under-detection is acceptable; the goal
/// here is to remove the false-positive on unrelated identifiers.
fn collect_identifier_annotations(src: &str) -> std::collections::HashMap<String, Vec<String>> {
    use std::collections::HashMap;
    // identifier is [A-Za-z_][A-Za-z0-9_]*
    // annotation captures non-greedy up to a delimiter.
    let re = regex::Regex::new(r"(?P<id>[A-Za-z_][A-Za-z0-9_]*)\s*:\s*(?P<ann>[^,)=\n;{]+)")
        .expect("static regex must compile");
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for line in src.lines() {
        let trimmed = line.trim();
        // Skip Python dict-literal / JSON-ish lines (`"key": value`) — those
        // are not type annotations. A line starting with a `"` or `'`
        // usually indicates a dict / mapping literal.
        if trimmed.starts_with('"') || trimmed.starts_with('\'') {
            continue;
        }
        for caps in re.captures_iter(line) {
            let id = caps.name("id").unwrap().as_str().to_string();
            let ann = caps.name("ann").unwrap().as_str().trim().to_string();
            // Skip noise: empty / annotation that is a bare keyword like "in".
            if ann.is_empty() {
                continue;
            }
            out.entry(id).or_default().push(ann);
        }
    }
    out
}

/// CB-006: classify `strong → weak` pairwise.
///
/// Returns `true` iff `before_ann` is a strictly stronger type than
/// `after_ann`. Conservative: unknown pairs return `false`.
fn is_annotation_weakening(before_ann: &str, after_ann: &str) -> bool {
    if before_ann == after_ann {
        return false;
    }
    let strong_to_weak: &[(&str, &[&str])] = &[
        // Python — concrete primitives → Any / object / generic dynamic.
        ("int", &["Any", "str", "object", "Optional[Any]"]),
        ("str", &["Any", "object", "Optional[Any]"]),
        ("bool", &["Any", "object", "int"]),
        ("float", &["Any", "object"]),
        // Rust — concrete → dynamic.
        (
            "&str",
            &["&dyn Any", "&dyn std::any::Any", "&dyn core::any::Any"],
        ),
        ("u32", &["Box<dyn Any>", "Box<dyn std::any::Any>"]),
        ("i32", &["Box<dyn Any>", "Box<dyn std::any::Any>"]),
    ];
    for (strong, weak_list) in strong_to_weak {
        if before_ann == *strong && weak_list.contains(&after_ann) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- SpecAuthority ordering / select -- //

    #[test]
    fn spec_authority_ordering_is_user_first_llm_last() {
        let mut variants = [
            SpecAuthority::LlmGeneratedTest,
            SpecAuthority::ImplementationContract,
            SpecAuthority::UserRequest,
            SpecAuthority::BehaviorContract,
        ];
        variants.sort();
        assert_eq!(variants[0], SpecAuthority::UserRequest);
        assert_eq!(variants[1], SpecAuthority::BehaviorContract);
        assert_eq!(variants[2], SpecAuthority::ImplementationContract);
        assert_eq!(variants[3], SpecAuthority::LlmGeneratedTest);
    }

    #[test]
    fn select_authority_returns_highest_priority_candidate() {
        let picked = select_authority(
            &[
                SpecAuthority::LlmGeneratedTest,
                SpecAuthority::UserRequest,
                SpecAuthority::ImplementationContract,
            ],
            None,
        );
        assert_eq!(picked, Some(SpecAuthority::UserRequest));
    }

    #[test]
    fn select_authority_ignores_verified_public_interface_as_dead_variant() {
        // Even when listed first, VerifiedPublicInterface must not be picked.
        let picked = select_authority(
            &[
                SpecAuthority::VerifiedPublicInterface,
                SpecAuthority::ImplementationContract,
            ],
            None,
        );
        assert_eq!(picked, Some(SpecAuthority::ImplementationContract));
    }

    #[test]
    fn select_authority_returns_none_when_only_dead_variant() {
        let picked = select_authority(&[SpecAuthority::VerifiedPublicInterface], None);
        assert!(picked.is_none());
    }

    #[test]
    fn select_authority_duplicate_top_variant_is_not_tie_cb001() {
        // CB-001 regression: [UserRequest, UserRequest] with a test/docs-only
        // consensus must NOT demote to LlmGeneratedTest. Duplicate same-variant
        // candidates are dedup'd before tie checks.
        let consensus = ArtifactConsensus::new(
            vec![ArtifactRole::Test, ArtifactRole::UsageDocs],
            vec![ArtifactRole::Implementation],
            "test+docs agree (impl dissents)",
        );
        let picked = select_authority(
            &[SpecAuthority::UserRequest, SpecAuthority::UserRequest],
            Some(&consensus),
        );
        assert_eq!(
            picked,
            Some(SpecAuthority::UserRequest),
            "duplicate UserRequest must not be demoted to a weaker authority"
        );
    }

    #[test]
    fn select_authority_consensus_cannot_demote_top_cb001() {
        // CB-001 guard: even if the consensus branch is somehow reached, the
        // consensus winner must not be weaker than the existing top.
        // Here `LlmGeneratedTest` is the bottom rank; consensus would suggest
        // `ImplementationContract` (a stronger authority), which is allowed.
        // Conversely, a stronger top (e.g., UserRequest) must never be demoted
        // even if consensus suggested `ImplementationContract`.
        let consensus_impl = ArtifactConsensus::new(
            vec![ArtifactRole::Implementation, ArtifactRole::UsageDocs],
            vec![ArtifactRole::Test],
            "impl and docs agree",
        );
        // UserRequest top + impl-consensus → top wins (no demotion).
        let picked = select_authority(
            &[SpecAuthority::UserRequest, SpecAuthority::UserRequest],
            Some(&consensus_impl),
        );
        assert_eq!(picked, Some(SpecAuthority::UserRequest));
    }

    #[test]
    fn select_authority_returns_none_for_empty_input() {
        assert_eq!(select_authority(&[], None), None);
    }

    // -- SF1: resolve() unit tests (Issue #647) -- //

    /// Helper: build a `SpecAuthorityInput` with all flags off and no
    /// consensus, so individual tests can flip exactly one field.
    fn empty_input() -> SpecAuthorityInput {
        SpecAuthorityInput {
            has_user_request_match: false,
            has_behavior_contract: false,
            has_verified_public_interface: false,
            is_newly_generated_task: false,
            consensus: None,
        }
    }

    #[test]
    fn sf1_resolve_user_request_wins_when_request_matches() {
        // SF1-resolve-user-request: any user-request match short-circuits
        // to the highest authority, even when other flags are on.
        let input = SpecAuthorityInput {
            has_user_request_match: true,
            has_behavior_contract: true,
            has_verified_public_interface: true,
            is_newly_generated_task: true,
            consensus: None,
        };
        assert_eq!(resolve(&input), SpecAuthority::UserRequest);
    }

    #[test]
    fn sf1_resolve_behavior_contract_when_only_contract_present() {
        // SF1-resolve-behavior-contract: BehaviorContract wins when only it
        // is set (user_request flag off).
        let input = SpecAuthorityInput {
            has_behavior_contract: true,
            ..empty_input()
        };
        assert_eq!(resolve(&input), SpecAuthority::BehaviorContract);
    }

    #[test]
    fn sf1_resolve_newly_generated_with_no_consensus_returns_llm_generated_test() {
        // SF1-resolve-newly-generated-equality: a brand-new task with no
        // consensus must NOT promote ImplementationContract — we elect the
        // lowest authority (LlmGeneratedTest) so the repair editor's
        // weakening detectors get to suppress test edits before impl wins.
        let input = SpecAuthorityInput {
            is_newly_generated_task: true,
            ..empty_input()
        };
        assert_eq!(resolve(&input), SpecAuthority::LlmGeneratedTest);
    }

    #[test]
    fn sf1_resolve_implementation_default_when_nothing_set() {
        // SF1-resolve-implementation-default: the empty fallback path
        // returns ImplementationContract (pre-SF1 production behavior).
        let input = empty_input();
        assert_eq!(resolve(&input), SpecAuthority::ImplementationContract);
    }

    #[test]
    fn sf1_resolve_consensus_tiebreak_when_consensus_present() {
        // SF1-resolve-consensus-tiebreak: when consensus is present (and the
        // newly-generated short-circuit does not fire), resolution delegates
        // to select_authority. Implementation-agreeing consensus elects
        // ImplementationContract via that path.
        let consensus = ArtifactConsensus::new(
            vec![ArtifactRole::Implementation, ArtifactRole::UsageDocs],
            vec![ArtifactRole::Test],
            "impl and docs agree",
        );
        let input = SpecAuthorityInput {
            consensus: Some(consensus),
            ..empty_input()
        };
        assert_eq!(resolve(&input), SpecAuthority::ImplementationContract);
    }

    #[test]
    fn sf1_resolve_verified_public_interface_when_only_that_flag_set() {
        // Forward-extensibility: even though the production caller passes
        // `false` for `has_verified_public_interface` (S5-005 dead-variant
        // policy), the resolver itself must respect the flag so a future
        // Issue wiring the session/turn-local cache can flip it.
        let input = SpecAuthorityInput {
            has_verified_public_interface: true,
            ..empty_input()
        };
        assert_eq!(resolve(&input), SpecAuthority::VerifiedPublicInterface);
    }

    #[test]
    fn sf1_resolve_newly_generated_short_circuits_before_consensus_only_when_consensus_absent() {
        // Boundary: newly_generated_task takes effect only when consensus
        // is None. If consensus is provided, we fall through to the
        // select_authority tie-break.
        let consensus = ArtifactConsensus::new(
            vec![ArtifactRole::Implementation],
            vec![ArtifactRole::Test],
            "impl alone",
        );
        let input = SpecAuthorityInput {
            is_newly_generated_task: true,
            consensus: Some(consensus),
            ..empty_input()
        };
        // Consensus is present → resolve() takes the tie-break path and
        // elects ImplementationContract, *not* LlmGeneratedTest.
        assert_eq!(resolve(&input), SpecAuthority::ImplementationContract);
    }

    // -- CB-008: consensus must not be bypassed by select_authority ordering -- //

    #[test]
    fn cb008_resolve_honors_consensus_when_test_and_usage_docs_agree_against_impl() {
        // CB-008 regression: test+usage_docs agreeing, impl dissenting.
        // Pre-fix: resolve() called select_authority(&[Impl, LlmGenTest], …)
        // which always returns Impl by ordered-enum top, silently bypassing
        // the consensus signal. Post-fix: the spec-side agreement (test +
        // docs) elects BehaviorContract so the repair editor cannot rewrite
        // the test to match buggy impl.
        let consensus = ArtifactConsensus::new(
            vec![ArtifactRole::Test, ArtifactRole::UsageDocs],
            vec![ArtifactRole::Implementation],
            "test and docs agree on spec",
        );
        let input = SpecAuthorityInput {
            consensus: Some(consensus),
            ..empty_input()
        };
        assert_eq!(resolve(&input), SpecAuthority::BehaviorContract);
    }

    #[test]
    fn cb008_resolve_honors_consensus_when_impl_and_usage_docs_agree_against_test() {
        // CB-008 regression: impl+usage_docs agreeing → ImplementationContract.
        // This is the most common 2-vs-1 case (stale test); the agreeing side
        // contains Implementation so the canonical mapping wins.
        let consensus = ArtifactConsensus::new(
            vec![ArtifactRole::Implementation, ArtifactRole::UsageDocs],
            vec![ArtifactRole::Test],
            "impl and docs agree",
        );
        let input = SpecAuthorityInput {
            consensus: Some(consensus),
            ..empty_input()
        };
        assert_eq!(resolve(&input), SpecAuthority::ImplementationContract);
    }

    #[test]
    fn cb008_resolve_falls_through_to_implementation_default_when_consensus_is_none() {
        // CB-008 regression: existing fallback behavior must be preserved
        // when consensus is None (and no higher-authority flag is set).
        let input = empty_input();
        assert_eq!(resolve(&input), SpecAuthority::ImplementationContract);
    }

    #[test]
    fn artifact_consensus_sanitizes_reason() {
        let c = ArtifactConsensus::new(
            vec![ArtifactRole::Implementation],
            vec![ArtifactRole::Test],
            "reason\x07with\x08ctl",
        );
        assert!(
            !c.reason.chars().any(|ch| (ch as u32) < 0x20),
            "control chars must be neutralized: {:?}",
            c.reason
        );
    }

    #[test]
    fn artifact_consensus_truncates_long_reason() {
        let raw = "x".repeat(MAX_CONSENSUS_REASON_CHARS + 50);
        let c = ArtifactConsensus::new(vec![ArtifactRole::Implementation], vec![], &raw);
        assert!(c.reason.chars().count() <= MAX_CONSENSUS_REASON_CHARS + 3); // +"..."
    }

    // -- SF1 V3.1: explicit-spec detector -- //

    #[test]
    fn sf1_v3_explicit_spec_positive_two_distinct_keywords() {
        // Two distinct prescriptive keywords ("must return", "should")
        // → user-request match fires.
        let request = "The endpoint must return 404 when the item is missing. \
                       The response should also include a JSON error body.";
        assert!(detect_explicit_spec_in_user_request(request));
    }

    #[test]
    fn sf1_v3_explicit_spec_positive_contract_marker() {
        let request = "Scoring contract: empty string is 0; add 1 point for each criterion.";
        assert!(detect_explicit_spec_in_user_request(request));
    }

    #[test]
    fn sf1_v3_explicit_spec_negative_single_should() {
        // A single "should" hit is below the threshold (2 distinct hits).
        let request = "You should probably add a test for the create flow.";
        assert!(!detect_explicit_spec_in_user_request(request));
    }

    #[test]
    fn sf1_v3_explicit_spec_negative_empty_request() {
        assert!(!detect_explicit_spec_in_user_request(""));
        assert!(!detect_explicit_spec_in_user_request("   \n  "));
    }

    #[test]
    fn sf1_v3_explicit_spec_positive_japanese_keywords() {
        // Two distinct Japanese prescriptive keywords are enough.
        let request = "本仕様の API は必ず 404 を返すこと。";
        assert!(detect_explicit_spec_in_user_request(request));
    }

    // -- Issue #647 CB-010: span-dedup against overlapping keyword hits -- //

    #[test]
    fn cb010_single_must_return_does_not_trigger_user_request_match() {
        // Regression: "must " (with trailing space) and "must return" both
        // appear in EXPLICIT_SPEC_KEYWORDS. Before span dedup, the single
        // phrase "must return 404" produced **two** hits and crossed the
        // 2-hit threshold — a false positive. After dedup the overlapping
        // spans collapse to one and the detector returns `false`.
        let request = "the endpoint must return 404 when item not found";
        assert!(!detect_explicit_spec_in_user_request(request));
    }

    #[test]
    fn cb010_two_distinct_keywords_still_trigger_match() {
        // Regression guard for CB-010 fix: two **non-overlapping** keyword
        // spans ("must " and "specification") must still cross the threshold.
        let request = "must return 404; this is the specification";
        assert!(detect_explicit_spec_in_user_request(request));
    }

    #[test]
    fn cb010_overlapping_must_and_must_return_dedup_to_one_hit() {
        // Pure span-dedup check: only "must " and "must return" overlap on
        // a single phrase, so the detector must see exactly one distinct
        // span and return `false`.
        let request = "must return 200";
        assert!(!detect_explicit_spec_in_user_request(request));
    }

    #[test]
    fn cb010_japanese_keyword_dedup() {
        // Japanese: "返すこと" is a substring of "返さなければならない"-style
        // phrases is not the case here, but "返すこと" alone, plus the
        // overlapping case where "仕様" appears only once, must not be
        // double-counted. A single Japanese phrase containing only
        // "返すこと" must NOT trigger the detector.
        let request = "API は 404 を返すこと。";
        assert!(!detect_explicit_spec_in_user_request(request));
    }

    // -- SF1 V3.3: consensus heuristic -- //

    #[test]
    fn sf1_v3_consensus_impl_and_docs_agree_test_dissents() {
        let consensus = detect_consensus_from_contract_conflict(
            "returns 404 when missing",
            "expects 200",
            "Returns 404 when missing",
        );
        let consensus = consensus.expect("two-vs-one agreement must yield consensus");
        assert!(consensus.agreeing.contains(&ArtifactRole::Implementation));
        assert!(consensus.agreeing.contains(&ArtifactRole::UsageDocs));
        assert_eq!(consensus.dissenting, vec![ArtifactRole::Test]);
    }

    #[test]
    fn sf1_v3_consensus_test_and_docs_agree_impl_dissents() {
        let consensus =
            detect_consensus_from_contract_conflict("returns 200", "expects 404", "expects 404");
        let consensus = consensus.expect("two-vs-one agreement must yield consensus");
        assert_eq!(consensus.dissenting, vec![ArtifactRole::Implementation]);
        assert!(consensus.agreeing.contains(&ArtifactRole::Test));
        assert!(consensus.agreeing.contains(&ArtifactRole::UsageDocs));
    }

    #[test]
    fn sf1_v3_consensus_all_three_agree_returns_none() {
        // Full agreement is not a tie-break signal.
        let consensus = detect_consensus_from_contract_conflict("foo", "foo", "foo");
        assert!(consensus.is_none());
    }

    #[test]
    fn sf1_v3_consensus_all_three_differ_returns_none() {
        let consensus = detect_consensus_from_contract_conflict("alpha", "beta", "gamma");
        assert!(consensus.is_none());
    }

    #[test]
    fn sf1_v3_consensus_empty_field_returns_none() {
        // If any field is empty after normalize, consensus can't fire.
        let consensus = detect_consensus_from_contract_conflict("foo", "foo", "");
        assert!(consensus.is_none());
    }

    #[test]
    fn sf1_v3_consensus_whitespace_and_case_insensitive() {
        // Normalize: lower-case + whitespace collapse → still agreement.
        let consensus =
            detect_consensus_from_contract_conflict("Returns   404", "expects 200", "returns 404");
        let consensus = consensus.expect("normalize must allow case/ws-tolerant match");
        assert!(consensus.agreeing.contains(&ArtifactRole::Implementation));
        assert!(consensus.agreeing.contains(&ArtifactRole::UsageDocs));
    }

    // -- SF1 V3.2: verified-public-interface from history -- //

    #[test]
    fn sf1_v3_verified_public_interface_false_by_default() {
        let hint = AgentHistoryHint::default();
        assert!(!detect_verified_public_interface_from_history(hint));
    }

    // CB-011: the prior `sf1_v3_verified_public_interface_true_when_verifier_passed_in_loop`
    // test asserted the detector returned `true` when `verifier_passed_in_loop = true`.
    // That route is now dead per CB-011 (artifact identity is not bound to the
    // current `SemanticFailureReport`), so the new behavior is exercised by
    // `cb011_verified_public_interface_detector_returns_false_when_artifact_identity_not_bound`
    // immediately below — there is no replacement positive test for the
    // pre-CB-011 path.

    // -- CB-011: VerifiedPublicInterface must stay a dead variant
    //    placeholder until artifact identity is bound to the failing
    //    `SemanticFailureReport`. The detector must NOT lift authority
    //    based on `verifier_passed_in_loop` alone (Issue #647 CB-011). --

    #[test]
    fn cb011_verified_public_interface_detector_returns_false_when_artifact_identity_not_bound() {
        // CB-011: even when the turn-local verifier-passed bit is set,
        // we have no signal proving the verified artifact matches the
        // *current* failure (SemanticFailureReport does not carry
        // path/interface identifiers). The detector must therefore
        // remain a dead placeholder and never lift the bit on its own.
        let hint = AgentHistoryHint {
            verifier_passed_in_loop: true,
        };
        assert!(
            !detect_verified_public_interface_from_history(hint),
            "verifier_passed_in_loop alone must not light VerifiedPublicInterface (CB-011)"
        );
    }

    #[test]
    fn cb011_resolve_does_not_elect_verified_public_interface_in_production() {
        // CB-011 integration: even if the production callsite passes a
        // hint where `verifier_passed_in_loop = true`, the detector
        // returns `false`, so the resulting `SpecAuthorityInput` has
        // `has_verified_public_interface = false` and `resolve` cannot
        // elect `VerifiedPublicInterface`. We exercise the detector +
        // resolve chain directly here (the production builder lives in
        // `turn.rs`; the parallel integration test lives there too).
        let hint = AgentHistoryHint {
            verifier_passed_in_loop: true,
        };
        let has_verified_public_interface = detect_verified_public_interface_from_history(hint);
        assert!(
            !has_verified_public_interface,
            "production hint with verifier_passed_in_loop=true must NOT light the flag"
        );
        let input = SpecAuthorityInput {
            has_user_request_match: false,
            has_behavior_contract: false,
            has_verified_public_interface,
            is_newly_generated_task: false,
            consensus: None,
        };
        // With no other signal lit, `resolve` falls through to the
        // ImplementationContract default — VerifiedPublicInterface is
        // unreachable in production.
        assert_ne!(
            resolve(&input),
            SpecAuthority::VerifiedPublicInterface,
            "VerifiedPublicInterface must stay a dead variant in production (CB-011)"
        );
        assert_eq!(resolve(&input), SpecAuthority::ImplementationContract);
    }

    // -- Test detectors -- //

    #[test]
    fn detect_assertion_deleted_positive() {
        let before = "def test_x():\n    assert foo() == 1\n    do_setup()\n";
        let after = "def test_x():\n    do_setup()\n";
        assert_eq!(
            detect_assertion_deleted("t.py", before, after),
            Some(WeakeningPattern::AssertionDeleted)
        );
    }

    #[test]
    fn detect_assertion_deleted_negative_when_assert_preserved() {
        let before = "def test_x():\n    assert foo() == 1\n";
        let after = "def test_x():\n    assert foo() == 1\n    extra()\n";
        assert!(detect_assertion_deleted("t.py", before, after).is_none());
    }

    #[test]
    fn detect_assertion_deleted_cb003_duplicate_row_one_removed() {
        // CB-003 regression: before has the same assert twice, after has it
        // once → 1 occurrence was deleted, AssertionDeleted must fire.
        let before = "def test_x():\n    assert foo() == 1\n    assert foo() == 1\n";
        let after = "def test_x():\n    assert foo() == 1\n";
        assert_eq!(
            detect_assertion_deleted("t.py", before, after),
            Some(WeakeningPattern::AssertionDeleted),
            "multiset semantics: deleting one of two duplicate asserts must fire"
        );
    }

    #[test]
    fn detect_skip_marker_added_positive_python() {
        let before = "def test_x():\n    assert foo()\n";
        let after = "@pytest.mark.skip\ndef test_x():\n    assert foo()\n";
        assert_eq!(
            detect_skip_marker_added("t.py", before, after),
            Some(WeakeningPattern::SkipMarkerAdded)
        );
    }

    #[test]
    fn detect_skip_marker_added_positive_rust() {
        let before = "#[test]\nfn test_x() { assert!(true); }\n";
        let after = "#[ignore]\n#[test]\nfn test_x() { assert!(true); }\n";
        assert_eq!(
            detect_skip_marker_added("t.rs", before, after),
            Some(WeakeningPattern::SkipMarkerAdded)
        );
    }

    #[test]
    fn detect_skip_marker_added_negative_when_unchanged() {
        let s = "fn test_x() {}\n";
        assert!(detect_skip_marker_added("t.rs", s, s).is_none());
    }

    #[test]
    fn detect_assert_true_weakening_positive() {
        let before = "def test_x():\n    assert foo() == 1\n";
        let after = "def test_x():\n    assert True\n";
        assert_eq!(
            detect_assert_true_weakening("t.py", before, after),
            Some(WeakeningPattern::AssertTrueWeakening)
        );
    }

    #[test]
    fn detect_assert_true_weakening_negative_when_no_prior_assert() {
        let before = "def test_x():\n    pass\n";
        let after = "def test_x():\n    pass\n    assert True\n";
        // No prior real assert → not a weakening of an existing assertion.
        assert!(detect_assert_true_weakening("t.py", before, after).is_none());
    }

    #[test]
    fn detect_test_function_deleted_positive_python() {
        let before = "def test_x():\n    assert foo()\n\ndef test_y():\n    assert bar()\n";
        let after = "def test_x():\n    assert foo()\n";
        assert_eq!(
            detect_test_function_deleted("t.py", before, after),
            Some(WeakeningPattern::TestFunctionDeleted)
        );
    }

    #[test]
    fn detect_test_function_deleted_positive_rust() {
        let before = "fn test_a() { assert!(true); }\nfn test_b() { assert!(true); }\n";
        let after = "fn test_a() { assert!(true); }\n";
        assert_eq!(
            detect_test_function_deleted("t.rs", before, after),
            Some(WeakeningPattern::TestFunctionDeleted)
        );
    }

    #[test]
    fn detect_literal_only_expected_change_positive() {
        let before = "def test_x():\n    x = compute()\n    assert x == 1\n";
        let after = "def test_x():\n    x = compute()\n    assert x == 99\n";
        assert_eq!(
            detect_literal_only_expected_change("t.py", before, after),
            Some(WeakeningPattern::LiteralOnlyExpectedChange)
        );
    }

    #[test]
    fn detect_literal_only_expected_change_negative_when_inputs_changed() {
        let before = "def test_x():\n    x = compute(1)\n    assert x == 1\n";
        let after = "def test_x():\n    x = compute(2)\n    assert x == 99\n";
        // Both inputs and expected changed → not "literal only".
        assert!(detect_literal_only_expected_change("t.py", before, after).is_none());
    }

    // -- Impl detectors -- //

    #[test]
    fn detect_validator_deleted_positive_assert() {
        let before = "fn run(x: i32) -> i32 {\n    assert!(x > 0);\n    x * 2\n}\n";
        let after = "fn run(x: i32) -> i32 {\n    x * 2\n}\n";
        assert_eq!(
            detect_validator_deleted("m.rs", before, after),
            Some(WeakeningPattern::ValidatorDeleted)
        );
    }

    #[test]
    fn detect_validator_deleted_positive_question_mark() {
        let before = "fn run() -> Result<i32, E> {\n    let x = step()?;\n    Ok(x)\n}\n";
        let after =
            "fn run() -> Result<i32, E> {\n    let x = step().unwrap_or_default();\n    Ok(x)\n}\n";
        assert_eq!(
            detect_validator_deleted("m.rs", before, after),
            Some(WeakeningPattern::ValidatorDeleted)
        );
    }

    #[test]
    fn detect_validator_deleted_negative() {
        let s = "fn run() -> i32 { 1 }\n";
        assert!(detect_validator_deleted("m.rs", s, s).is_none());
    }

    #[test]
    fn detect_validator_deleted_cb004_python_multiline_raise() {
        // CB-004 acceptance: multiline validator (`if invalid:` next line
        // `raise ValueError`) where only the `raise` line is deleted must
        // fire as ValidatorDeleted.
        let before = "def run(x):\n    if not x:\n        raise ValueError('bad')\n    return x\n";
        let after = "def run(x):\n    if not x:\n        pass\n    return x\n";
        assert_eq!(
            detect_validator_deleted("m.py", before, after),
            Some(WeakeningPattern::ValidatorDeleted)
        );
    }

    #[test]
    fn detect_validator_deleted_cb004_rust_return_err_deleted() {
        // CB-004: `return Err(...)` deletion is a validator removal.
        let before = "fn run(x: i32) -> Result<i32, E> {\n    if x < 0 {\n        return Err(E::Neg);\n    }\n    Ok(x)\n}\n";
        let after = "fn run(x: i32) -> Result<i32, E> {\n    if x < 0 {\n    }\n    Ok(x)\n}\n";
        assert_eq!(
            detect_validator_deleted("m.rs", before, after),
            Some(WeakeningPattern::ValidatorDeleted)
        );
    }

    #[test]
    fn detect_validator_deleted_cb004_rust_bail_deleted() {
        // CB-004: bail!() terminal removed → ValidatorDeleted.
        let before = "fn run(x: i32) -> anyhow::Result<i32> {\n    if x < 0 {\n        bail!(\"negative\");\n    }\n    Ok(x)\n}\n";
        let after =
            "fn run(x: i32) -> anyhow::Result<i32> {\n    if x < 0 {\n    }\n    Ok(x)\n}\n";
        assert_eq!(
            detect_validator_deleted("m.rs", before, after),
            Some(WeakeningPattern::ValidatorDeleted)
        );
    }

    #[test]
    fn detect_validator_deleted_cb004_ts_throw_deleted() {
        // CB-004: `throw new ...` terminal removed → ValidatorDeleted.
        let before = "function run(x: number) {\n  if (x < 0) {\n    throw new Error('neg');\n  }\n  return x;\n}\n";
        let after = "function run(x: number) {\n  if (x < 0) {\n  }\n  return x;\n}\n";
        assert_eq!(
            detect_validator_deleted("m.ts", before, after),
            Some(WeakeningPattern::ValidatorDeleted)
        );
    }

    #[test]
    fn detect_error_swallowed_positive_python() {
        let before = "def run():\n    do_thing()\n";
        let after = "def run():\n    try:\n        do_thing()\n    except: pass\n";
        assert_eq!(
            detect_error_swallowed("m.py", before, after),
            Some(WeakeningPattern::ErrorSwallowed)
        );
    }

    #[test]
    fn detect_error_swallowed_positive_rust_ok() {
        let before = "fn run() -> Result<(), E> {\n    step()?;\n    Ok(())\n}\n";
        let after = "fn run() -> Result<(), E> {\n    step().ok();\n    Ok(())\n}\n";
        assert_eq!(
            detect_error_swallowed("m.rs", before, after),
            Some(WeakeningPattern::ErrorSwallowed)
        );
    }

    #[test]
    fn detect_error_swallowed_negative_when_propagation_kept() {
        let s = "fn run() -> Result<(), E> { step()?; Ok(()) }\n";
        assert!(detect_error_swallowed("m.rs", s, s).is_none());
    }

    #[test]
    fn detect_early_return_bypass_positive() {
        let before = "fn run(flag: bool) -> i32 {\n    let x = compute();\n    x\n}\n";
        let after =
            "fn run(flag: bool) -> i32 {\n    return Ok(());\n    let x = compute();\n    x\n}\n";
        assert_eq!(
            detect_early_return_bypass("m.rs", before, after),
            Some(WeakeningPattern::EarlyReturnBypass)
        );
    }

    #[test]
    fn detect_early_return_bypass_negative_when_no_added_return() {
        let s = "fn run() -> i32 { compute() }\n";
        assert!(detect_early_return_bypass("m.rs", s, s).is_none());
    }

    #[test]
    fn detect_type_annotation_weakened_python_same_ident() {
        // CB-006: same identifier `x` moved from `int` to `Any` → fire.
        let before = "def f(x: int) -> int:\n    return x\n";
        let after = "def f(x: Any) -> int:\n    return x\n";
        assert_eq!(
            detect_type_annotation_weakened("m.py", before, after),
            Some(WeakeningPattern::TypeAnnotationWeakened)
        );
    }

    #[test]
    fn detect_type_annotation_weakened_rust_same_ident() {
        // CB-006: same param `x` moved from `&str` to `&dyn Any` → fire.
        let before = "fn run(x: &str) -> String { x.to_owned() }\n";
        let after = "fn run(x: &dyn Any) -> String { format!(\"{:?}\", x) }\n";
        assert_eq!(
            detect_type_annotation_weakened("m.rs", before, after),
            Some(WeakeningPattern::TypeAnnotationWeakened)
        );
    }

    #[test]
    fn detect_type_annotation_weakened_negative() {
        let s = "fn run(x: &str) -> String { x.to_owned() }\n";
        assert!(detect_type_annotation_weakened("m.rs", s, s).is_none());
    }

    #[test]
    fn detect_type_annotation_weakened_cb006_item_id_int_to_str() {
        // CB-006 acceptance (a): item_id: int → item_id: str must fire.
        let before = "def f(item_id: int) -> int:\n    return item_id\n";
        let after = "def f(item_id: str) -> int:\n    return item_id\n";
        assert_eq!(
            detect_type_annotation_weakened("m.py", before, after),
            Some(WeakeningPattern::TypeAnnotationWeakened)
        );
    }

    #[test]
    fn detect_type_annotation_weakened_cb006_unrelated_any_no_fire() {
        // CB-006 acceptance (b): adding an unrelated `unrelated: Any`
        // variable must NOT fire — only existing identifiers count.
        let before = "def f(item_id: int) -> int:\n    return item_id\n";
        let after = "def f(item_id: int) -> int:\n    unrelated: Any = 0\n    return item_id\n";
        assert!(
            detect_type_annotation_weakened("m.py", before, after).is_none(),
            "unrelated `: Any` addition must not be reported as weakening"
        );
    }

    // -- Orchestration -- //

    #[test]
    fn detect_test_weakening_aggregates_independent_detectors() {
        // Both AssertionDeleted and SkipMarkerAdded fire at once.
        let before = "def test_x():\n    assert foo() == 1\n";
        let after = "@pytest.mark.skip\ndef test_x():\n    pass\n";
        let mut patterns = detect_test_weakening("t.py", before, after);
        patterns.sort_by_key(|p| format!("{p:?}"));
        assert!(patterns.contains(&WeakeningPattern::AssertionDeleted));
        assert!(patterns.contains(&WeakeningPattern::SkipMarkerAdded));
    }

    #[test]
    fn detect_test_weakening_empty_for_identical_input() {
        let s = "def test_x():\n    assert foo() == 1\n";
        assert!(detect_test_weakening("t.py", s, s).is_empty());
    }

    #[test]
    fn detect_impl_weakening_aggregates_independent_detectors() {
        let before = "fn run() -> Result<i32, E> {\n    let x = step()?;\n    Ok(x)\n}\n";
        let after =
            "fn run() -> Result<i32, E> {\n    step().ok();\n    return Ok(());\n    Ok(0)\n}\n";
        let patterns = detect_impl_weakening("m.rs", before, after);
        assert!(patterns.contains(&WeakeningPattern::ValidatorDeleted));
        assert!(patterns.contains(&WeakeningPattern::ErrorSwallowed));
        assert!(patterns.contains(&WeakeningPattern::EarlyReturnBypass));
    }

    #[test]
    fn detect_impl_weakening_empty_for_identical_input() {
        let s = "fn run() -> i32 { 1 }\n";
        assert!(detect_impl_weakening("m.rs", s, s).is_empty());
    }

    // -- RepairRole alias -- //

    #[test]
    fn repair_role_is_artifact_role_alias() {
        // Same type → assignable both ways without conversion.
        let r: RepairRole = ArtifactRole::Implementation;
        let a: ArtifactRole = r;
        assert_eq!(a, ArtifactRole::Implementation);
    }

    // -- Phase G grep / structure tests (Issue #647 acceptance closure) -- //

    /// Helper: split the module source into the production prefix
    /// (everything before `#[cfg(test)]\nmod tests`).
    fn production_source() -> &'static str {
        let source = include_str!("spec_authority.rs");
        match source.find("#[cfg(test)]\nmod tests") {
            Some(idx) => &source[..idx],
            None => source,
        }
    }

    #[test]
    fn spec_authority_enum_has_exactly_5_variants() {
        // S1-005: SpecAuthority is fixed to 5 variants — adding a sixth
        // variant must be a deliberate design decision and force this
        // test to be updated (which serves as a review trigger).
        let all = [
            SpecAuthority::UserRequest,
            SpecAuthority::BehaviorContract,
            SpecAuthority::VerifiedPublicInterface,
            SpecAuthority::ImplementationContract,
            SpecAuthority::LlmGeneratedTest,
        ];
        // Each variant matches exactly itself — exhaustive `match` here
        // is the structural lock: if a 6th variant is added, the `match`
        // below becomes non-exhaustive and the test fails to compile.
        for v in &all {
            let label: &'static str = match v {
                SpecAuthority::UserRequest => "user_request",
                SpecAuthority::BehaviorContract => "behavior_contract",
                SpecAuthority::VerifiedPublicInterface => "verified_public_interface",
                SpecAuthority::ImplementationContract => "implementation_contract",
                SpecAuthority::LlmGeneratedTest => "llm_generated_test",
            };
            assert!(!label.is_empty());
        }
        assert_eq!(all.len(), 5);
    }

    #[test]
    fn no_framework_literal_in_spec_authority_module() {
        // S1-012 (拡張): new production code must not embed
        // framework-specific literals.
        let prod = production_source();
        for lit in &["\"422\"", "\"404\"", "/items/nonexistent", "FastAPI"] {
            assert!(
                !prod.contains(lit),
                "spec_authority.rs production code must not contain framework literal {lit:?}",
            );
        }
    }

    #[test]
    fn no_unsafe_in_spec_authority_module() {
        // DR4-003: no unsafe / FFI in new production code.
        let prod = production_source();
        assert!(
            !prod.contains("unsafe "),
            "spec_authority.rs production code must not contain `unsafe `",
        );
        assert!(
            !prod.contains("extern \"C\""),
            "spec_authority.rs production code must not declare FFI",
        );
    }

    #[test]
    fn photon_layer_does_not_import_semantic_repair_types() {
        // DR3-002: photon → session → agent is the only allowed direction.
        // The semantic-repair planner lives in `agent/loop_run/` and must
        // never leak into the photon sidecar layer.
        let photon_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/photon");
        assert!(
            photon_dir.is_dir(),
            "src/photon must exist for DR3-002 to be meaningful",
        );
        let mut visited_files = 0usize;
        let entries =
            std::fs::read_dir(&photon_dir).expect("read_dir src/photon for DR3-002 grep test");
        for entry in entries {
            let entry = entry.expect("photon dir entry");
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("rs") {
                continue;
            }
            let body =
                std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
            for forbidden in &[
                "SemanticFailureReport",
                "SpecAuthority",
                "FailureCluster",
                "FailureClusterKey",
                "semantic_failure",
                "spec_authority",
            ] {
                assert!(
                    !body.contains(forbidden),
                    "DR3-002 violated: {path:?} contains forbidden symbol {forbidden:?}",
                );
            }
            visited_files += 1;
        }
        assert!(
            visited_files > 0,
            "photon layer scan must visit at least one .rs file"
        );
    }
}
