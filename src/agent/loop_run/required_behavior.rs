//! `RequiredBehaviorContract` — minimal schema and deterministic extractor for
//! user-request behavior contracts (Issue #635).
//!
//! ## Layer / scope (CLAUDE.md DR3-001 / DR3-002)
//!
//! - All types and helpers are kept at `pub(super)` (`pub(crate)` for the few
//!   that need to cross `task_contract.rs`). The parent module
//!   `src/agent/loop_run.rs` MUST NOT `pub use` anything from here.
//! - Direction of dependency is `agent → session` only. We import
//!   [`crate::session::feedback::mask_secrets`] as the **SSOT** for redacting
//!   request-derived text before it ever lands in a schema field
//!   (Security Invariants in CLAUDE.md).
//! - The extractor is a **pure function**: no LLM call, no I/O, no access to
//!   `Agent` / `SessionSnapshot` / log payloads.
//!
//! ## Two-stage pipeline
//!
//! 1. [`extract`] builds a candidate [`RequiredBehaviorContract`] from
//!    deterministic keyword / token rules over a bounded, masked scan of
//!    the request text. Its responsibility is **discovery** — turning a
//!    free-form request into closed-enum / cap-bounded fields.
//! 2. [`filter_against_request`] re-validates each candidate field against
//!    the same request text, dropping any term that is not literally backed
//!    by the bounded masked scan. Its responsibility is **trust narrowing**:
//!    the candidate is treated as *untrusted* (as if it had come from an
//!    LLM), so [`MAX_ARRAY`] / [`MAX_STR`] / secret redaction are reapplied,
//!    `required_artifacts` is intersected with a freshly-derived
//!    request-backed set (CB-005), and `confidence` is recomputed.
//!
//! In Issue #635 scope the two stages share the same input source, so the
//! filter is effectively a no-op on candidates produced by [`extract`]. The
//! shape exists so a future LLM-backed candidate generator can be slotted
//! in at stage 1 without weakening the request-backing guarantee.
//!
//! A third helper, [`RequiredBehaviorContract::to_artifact_roles`], is a
//! pure **projection** of the schema's `required_artifacts` into the
//! [`ArtifactRole`] SSOT consumed by completion gates. It is intentionally
//! decoupled from extraction / filtering so Issue #636 can swap the
//! contract's artifact source without touching either pipeline stage.

use super::task_contract::ArtifactRole;

/// Upper bound for the byte slice of `request` that the extractor / filter
/// will scan (DR4-002). Both raw user request and any candidate string get
/// truncated to this many UTF-8 bytes (char boundary preserved) before any
/// keyword / token scan or `to_ascii_lowercase` allocation. The cap protects
/// against DoS-style long inputs without touching the original
/// `Agent::messages` content.
pub(super) const MAX_REQUEST_SCAN_BYTES: usize = 64 * 1024;

/// Maximum number of items kept in any `Option<Vec<_>>` field.
const MAX_ARRAY: usize = 8;

/// Maximum number of bytes kept per `domain_terms` string.
const MAX_STR: usize = 64;

/// Issue #665: Maximum bytes for `BoundedLabelWithExcerpt::label`. Re-uses the
/// existing `MAX_STR` cap to keep all bounded-label fields under the same
/// limit (design policy §3-5 SSOT).
pub(super) const LABEL_MAX_LEN: usize = MAX_STR;

/// Issue #665: Maximum bytes for `BoundedLabelWithExcerpt::excerpt`. Set to
/// `LABEL_MAX_LEN * 4` to allow longer raw user-request fragments while
/// keeping `behavior_contract` JSON payload bounded.
pub(super) const EXCERPT_MAX_LEN: usize = LABEL_MAX_LEN * 4;

/// Issue #665: Confidence threshold below which
/// `project_behavior_contract` returns `None` (the projection is dropped
/// rather than fed to diagnostic / repair prompts). SSOT for the
/// "low confidence skip" invariant.
///
/// `#[allow(dead_code)]` is intentional until Phase 4 lands the
/// `project_behavior_contract` accessor that consumes this threshold.
#[allow(dead_code)]
pub(super) const LOW_CONFIDENCE_THRESHOLD: f32 = 0.5;

/// Issue #665: Serialized payload size cap for `behavior_contract` JSON
/// injected into diagnostic / repair prompts. Cap super-set: low-priority
/// field drop + `truncated=true` metadata when exceeded.
///
/// `#[allow(dead_code)]` is intentional until Phase 5 lands the
/// `verifier_diagnostic_messages` signature extension that enforces this cap.
#[allow(dead_code)]
pub(super) const MAX_BEHAVIOR_CONTRACT_PROJECTION_BYTES: usize = 1200;

/// Issue #665 (S5-005 / S7-003): build the `behavior_contract` JSON value to
/// inject into diagnostic / repair user-message payloads. Caller-side
/// projection is taken as-is when present, then the **serialized** size is
/// capped at [`MAX_BEHAVIOR_CONTRACT_PROJECTION_BYTES`]. When the cap is
/// exceeded the low-priority fields are dropped in this order:
/// 1. `non_goals` (lowest priority — diagnostic prompts care less about
///    "do not")
/// 2. `verification_expectations`
/// 3. `required_capabilities`
/// 4. `behavior_goal` (highest priority — kept whenever possible)
///
/// If even the smallest envelope exceeds the cap, returns `null`. A
/// `truncated=true` metadata key is added when any field was dropped.
/// Returns `serde_json::Value::Null` when `projection` is `None`.
pub(super) fn behavior_contract_payload_value(
    projection: Option<&BehaviorContractProjection>,
) -> serde_json::Value {
    let Some(proj) = projection else {
        return serde_json::Value::Null;
    };
    fn label_excerpt_value(item: &BoundedLabelWithExcerpt) -> serde_json::Value {
        match item.excerpt.as_ref() {
            Some(ex) => serde_json::json!({"label": item.label, "excerpt": ex}),
            None => serde_json::json!({"label": item.label}),
        }
    }
    fn vec_value(items: &[BoundedLabelWithExcerpt]) -> serde_json::Value {
        serde_json::Value::Array(items.iter().map(label_excerpt_value).collect())
    }
    // Issue #665 (CB-003): char-boundary safe truncate of excerpts to
    // EXCERPT_MAX_LEN / 2 before drop-order escalation. This keeps the
    // most informative metadata field (behavior_goal) intact while still
    // allowing the cap to be satisfied via shorter excerpts.
    fn truncate_excerpt_char_safe(s: &str, target: usize) -> String {
        if s.len() <= target {
            return s.to_string();
        }
        let mut end = target.min(s.len());
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        s[..end].to_string()
    }
    fn label_excerpt_value_truncated(
        item: &BoundedLabelWithExcerpt,
        excerpt_cap: usize,
    ) -> serde_json::Value {
        match item.excerpt.as_ref() {
            Some(ex) => {
                let truncated = truncate_excerpt_char_safe(ex, excerpt_cap);
                serde_json::json!({"label": item.label, "excerpt": truncated})
            }
            None => serde_json::json!({"label": item.label}),
        }
    }
    fn vec_value_truncated(
        items: &[BoundedLabelWithExcerpt],
        excerpt_cap: usize,
    ) -> serde_json::Value {
        serde_json::Value::Array(
            items
                .iter()
                .map(|i| label_excerpt_value_truncated(i, excerpt_cap))
                .collect(),
        )
    }

    let mut behavior_goal = proj.behavior_goal.as_ref().map(label_excerpt_value);
    let mut required_capabilities = vec_value(&proj.required_capabilities);
    let mut verification_expectations = vec_value(&proj.verification_expectations);
    let mut non_goals = vec_value(&proj.non_goals);
    let mut truncated = false;
    let cap = MAX_BEHAVIOR_CONTRACT_PROJECTION_BYTES;
    let half_excerpt_cap = EXCERPT_MAX_LEN / 2;

    fn assemble(
        confidence: f32,
        fields_used: &[&'static str],
        behavior_goal: &Option<serde_json::Value>,
        required_capabilities: &serde_json::Value,
        verification_expectations: &serde_json::Value,
        non_goals: &serde_json::Value,
        truncated: bool,
    ) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert("confidence".to_string(), serde_json::json!(confidence));
        obj.insert("fields_used".to_string(), serde_json::json!(fields_used));
        if let Some(g) = behavior_goal {
            obj.insert("behavior_goal".to_string(), g.clone());
        } else {
            obj.insert("behavior_goal".to_string(), serde_json::Value::Null);
        }
        obj.insert(
            "required_capabilities".to_string(),
            required_capabilities.clone(),
        );
        obj.insert(
            "verification_expectations".to_string(),
            verification_expectations.clone(),
        );
        obj.insert("non_goals".to_string(), non_goals.clone());
        if truncated {
            obj.insert("truncated".to_string(), serde_json::Value::Bool(true));
        }
        serde_json::Value::Object(obj)
    }

    let serialize_size = |v: &serde_json::Value| -> usize {
        serde_json::to_string(v)
            .map(|s| s.len())
            .unwrap_or(usize::MAX)
    };

    let mut value = assemble(
        proj.confidence,
        &proj.fields_used,
        &behavior_goal,
        &required_capabilities,
        &verification_expectations,
        &non_goals,
        truncated,
    );
    if serialize_size(&value) <= cap {
        return value;
    }
    // 0. (CB-003) Try truncating all excerpts to EXCERPT_MAX_LEN / 2 first.
    //    This is a softer reduction than full field drops.
    behavior_goal = proj
        .behavior_goal
        .as_ref()
        .map(|i| label_excerpt_value_truncated(i, half_excerpt_cap));
    required_capabilities = vec_value_truncated(&proj.required_capabilities, half_excerpt_cap);
    verification_expectations =
        vec_value_truncated(&proj.verification_expectations, half_excerpt_cap);
    non_goals = vec_value_truncated(&proj.non_goals, half_excerpt_cap);
    truncated = true;
    value = assemble(
        proj.confidence,
        &proj.fields_used,
        &behavior_goal,
        &required_capabilities,
        &verification_expectations,
        &non_goals,
        truncated,
    );
    if serialize_size(&value) <= cap {
        return value;
    }
    // 1. Drop non_goals
    non_goals = serde_json::Value::Array(vec![]);
    value = assemble(
        proj.confidence,
        &proj.fields_used,
        &behavior_goal,
        &required_capabilities,
        &verification_expectations,
        &non_goals,
        truncated,
    );
    if serialize_size(&value) <= cap {
        return value;
    }
    // 2. Drop verification_expectations
    verification_expectations = serde_json::Value::Array(vec![]);
    value = assemble(
        proj.confidence,
        &proj.fields_used,
        &behavior_goal,
        &required_capabilities,
        &verification_expectations,
        &non_goals,
        truncated,
    );
    if serialize_size(&value) <= cap {
        return value;
    }
    // 3. Drop required_capabilities
    required_capabilities = serde_json::Value::Array(vec![]);
    value = assemble(
        proj.confidence,
        &proj.fields_used,
        &behavior_goal,
        &required_capabilities,
        &verification_expectations,
        &non_goals,
        truncated,
    );
    if serialize_size(&value) <= cap {
        return value;
    }
    // 4. Drop behavior_goal
    behavior_goal = None;
    value = assemble(
        proj.confidence,
        &proj.fields_used,
        &behavior_goal,
        &required_capabilities,
        &verification_expectations,
        &non_goals,
        truncated,
    );
    if serialize_size(&value) <= cap {
        return value;
    }
    // Even the smallest envelope is over cap — return null (better than an
    // attacker-controlled un-bounded payload).
    serde_json::Value::Null
}

/// Redaction sentinels emitted by [`mask_secrets`] / related helpers. Any
/// candidate term containing one of these is dropped from `domain_terms`.
const REDACTION_SENTINELS: &[&str] = &["***", "<REDACTED>"];

/// Behavior contract extracted from a user request.
///
/// `confidence` is an `f32`, so this type intentionally only derives
/// `PartialEq` (not `Eq`). `None` represents an unknown / not-extracted
/// field and is distinct from `Some(vec![])` (extracted but empty).
///
/// `#[derive(Deserialize)]` is **not** applied in Issue #635 scope: the
/// only input path is deterministic extraction from `request: &str`. A
/// Deserialize derive would invite JSON-shaped inputs from the session
/// layer (DR3-002 risk). LLM extensions can add the derive when a JSON
/// path actually exists.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RequiredBehaviorContract {
    pub(super) operations: Option<Vec<Operation>>,
    pub(super) domain_terms: Option<Vec<String>>,
    pub(super) interface_hints: Option<Vec<InterfaceHint>>,
    pub(super) required_artifacts: Option<Vec<ArtifactKind>>,
    pub(super) verification: Option<Vec<VerificationKind>>,
    pub(super) confidence: f32,
    /// Issue #651: hard gate that says "the user request literally asked for
    /// test execution evidence". SSOT predicate is
    /// `super::task_contract::request_asks_for_test_artifact`, computed once
    /// per `extract` / `filter_against_request` call against the bounded
    /// masked scan. Distinct from `verification.contains(VerificationKind::Test)`
    /// — that one only fires on the English `test` keyword, missing
    /// `spec` / `テストも実装` (design judgement #1).
    ///
    /// `Default` is intentionally NOT implemented for this struct;
    /// every literal construction site (now 10 across this module + 1 in
    /// `artifact_ledger.rs`) is updated explicitly so adding a new field
    /// stays compile-time visible (design policy DR2-006 / DR2-008).
    pub(super) test_execution_required: bool,
    /// Issue #665: 1-line summary of the user-stated goal (label + raw
    /// excerpt). `None` when extraction does not find a deterministic
    /// match. `behavior_goal` is the only singular-Option field of the
    /// 4 new fields; the other 3 are `Vec<...>`.
    pub(super) behavior_goal: Option<BoundedLabelWithExcerpt>,
    /// Issue #665: Bounded labels for required capabilities. Derived from
    /// `operations` + `domain_terms` (post-filter source). `excerpt` is
    /// `None` by policy (S5-003): only `behavior_goal` / `non_goals` carry
    /// raw excerpts.
    pub(super) required_capabilities: Option<Vec<BoundedLabelWithExcerpt>>,
    /// Issue #665: Bounded labels for verification expectations. Derived
    /// from `verification` (post-filter source). `excerpt` is `None` by
    /// policy (S5-003).
    pub(super) verification_expectations: Option<Vec<BoundedLabelWithExcerpt>>,
    /// Issue #665: Bounded labels + raw excerpts for non-goals (e.g.
    /// "do not", "X はしない"). Like `behavior_goal`, this field carries
    /// raw excerpts.
    pub(super) non_goals: Option<Vec<BoundedLabelWithExcerpt>>,
}

/// Issue #665: Bounded label + optional raw excerpt value object. Common
/// shape for the 4 new fields on [`RequiredBehaviorContract`].
///
/// - `label`: bounded string (≤ [`LABEL_MAX_LEN`] bytes) derived from a
///   closed enum or keyword scan.
/// - `excerpt`: optional raw user-request fragment (≤ [`EXCERPT_MAX_LEN`]
///   bytes), already passed through [`mask_secrets`] / [`mask_header_family`]
///   via [`bounded_masked_request`] at the extraction site. `None` for
///   derived fields (`required_capabilities` / `verification_expectations`)
///   per design policy S5-003.
///
/// `Default` is intentionally NOT implemented (DR2-006 / DR2-008) — every
/// literal construction site must spell out the value explicitly so adding
/// a new field stays compile-time visible.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct BoundedLabelWithExcerpt {
    pub(super) label: String,
    pub(super) excerpt: Option<String>,
}

/// Issue #665 — Phase 4: `RequiredBehaviorContract` の diagnostic / repair
/// 向け縮退表現（sidecar projection）。
///
/// `RepairJob` / `VerifierRepairAssessment` に field として保持しない
/// turn-local computed value（prompt 組立点で都度 build）。命名は
/// consumer-neutral (`BehaviorContract` prefix) — 同一 projection を
/// `verifier_diagnostic_messages` / `verifier_repair_pass_messages` /
/// 将来の `artifact_completion` 等の複数 consumer が読み得る (DR1-010)。
///
/// `Eq` は派生しない (`confidence: f32` を含む可能性)。
/// `Serialize` は本 Issue 範囲外。
///
/// **field 設計の非対称性 (設計判断 #7)**: `behavior_goal` のみ singular
/// `Option<...>`、他 3 フィールドは `Vec<...>`。
///
/// `#[allow(dead_code)]` は Phase 5 (turn.rs caller wiring) で
/// consumer が landing するまで dead_code 警告を抑制。
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub(super) struct BehaviorContractProjection {
    pub(super) confidence: f32,
    /// 走査順は `FIELDS_USED_ORDER` 由来で deterministic (DR1-005)。
    pub(super) fields_used: Vec<&'static str>,
    pub(super) behavior_goal: Option<BoundedLabelWithExcerpt>,
    pub(super) required_capabilities: Vec<BoundedLabelWithExcerpt>,
    pub(super) verification_expectations: Vec<BoundedLabelWithExcerpt>,
    pub(super) non_goals: Vec<BoundedLabelWithExcerpt>,
}

/// Issue #665 — Phase 6 / S5-006 / S7-002: payload-shaped dedup key for the
/// `agent.behavior_contract.projected` observability event.
///
/// The key contains **only metadata** that the emitted log event payload
/// actually keys on:
/// - `schema_version`: matches the envelope `schema_version: 1`.
/// - `consumer`: which prompt site consumed the projection.
/// - `confidence_bucket`: `(confidence * 10.0)` clamped to `[0, 10]` u8.
/// - `fields_used`: deterministic-order vector of `&'static str` from
///   `FIELDS_USED_ORDER`.
///
/// **Why no raw label / excerpt (S5-006)**: dedup state lives on `Agent`
/// across turn-boundary work, so retaining attacker-controlled text would
/// widen the trust surface. The bucket / fields_used signal is enough to
/// suppress duplicate emissions within a turn.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BehaviorProjectionEventKey {
    pub(super) schema_version: u32,
    pub(super) consumer: &'static str,
    pub(super) confidence_bucket: u8,
    pub(super) fields_used: Vec<&'static str>,
}

impl BehaviorProjectionEventKey {
    /// Issue #665 — Phase 6: SSOT bucketization of `confidence` into a u8
    /// in `[0, 10]`. Non-finite values clamp to 0.
    pub(super) fn bucket_from_confidence(confidence: f32) -> u8 {
        if !confidence.is_finite() {
            return 0;
        }
        let scaled = confidence * 10.0;
        if scaled <= 0.0 {
            0
        } else if scaled >= 10.0 {
            10
        } else {
            scaled.round() as u8
        }
    }

    /// Issue #665 — Phase 6: build a key from a projection + consumer label.
    pub(super) fn from_projection(
        projection: &BehaviorContractProjection,
        consumer: &'static str,
    ) -> Self {
        Self {
            schema_version: BEHAVIOR_CONTRACT_PROJECTED_SCHEMA_VERSION,
            consumer,
            confidence_bucket: Self::bucket_from_confidence(projection.confidence),
            fields_used: projection.fields_used.clone(),
        }
    }
}

/// Issue #665 — Phase 6 / S7-002: schema_version of the
/// `agent.behavior_contract.projected` event payload. Bumped when payload
/// shape changes incompatibly so #666 can `join` / `ignore` old sessions.
pub(super) const BEHAVIOR_CONTRACT_PROJECTED_SCHEMA_VERSION: u32 = 1;

/// Issue #665: allowlist of `consumer` strings for the
/// `agent.behavior_contract.projected` event.
#[allow(dead_code)]
pub(super) const BEHAVIOR_CONTRACT_CONSUMER_VERIFIER_DIAGNOSTIC: &str = "verifier_diagnostic";

/// Issue #665: allowlist of `consumer` strings for the
/// `agent.behavior_contract.projected` event.
#[allow(dead_code)]
pub(super) const BEHAVIOR_CONTRACT_CONSUMER_VERIFIER_REPAIR: &str = "verifier_repair";

/// Issue #665 — Phase 6 / S5-006: payload builder for the
/// `agent.behavior_contract.projected` event. Returns a `serde_json::Value`
/// (object) containing **only** non-PII metadata. Raw `label` / `excerpt`
/// must never appear in event payloads (Security Invariants).
pub(super) fn behavior_contract_projected_payload(
    key: &BehaviorProjectionEventKey,
    confidence: f32,
    session_id: &str,
    turn_index: u64,
) -> serde_json::Value {
    serde_json::json!({
        "schema_version": key.schema_version,
        "session_id": session_id,
        "turn_index": turn_index,
        "consumer": key.consumer,
        "confidence": confidence,
        "fields_used": key.fields_used,
    })
}

/// Issue #665: deterministic ordering of `fields_used` strings emitted by
/// [`project_behavior_contract`]. Keeping this in a `const` slice provides
/// SSOT for both the projection helper and any future observability
/// assertion tests (DR1-005).
#[allow(dead_code)]
pub(super) const FIELDS_USED_ORDER: &[&str] = &[
    "behavior_goal",
    "required_capabilities",
    "verification_expectations",
    "non_goals",
];

/// Issue #665 — Phase 4 / S5-001: Build a [`BehaviorContractProjection`] from
/// a [`super::task_contract::TaskContract`].
///
/// Returns `None` when:
/// - `contract.required_behavior.confidence < LOW_CONFIDENCE_THRESHOLD`,
/// - or all 4 new fields are unset (no consumable data).
///
/// Pure fn / no I/O / no Agent state. Safe to call in deeply nested prompt
/// build sites.
///
/// `#[allow(dead_code)]` は Phase 5 で `turn.rs` caller が landing するまで
/// dead_code 警告を抑制。
#[allow(dead_code)]
pub(super) fn project_behavior_contract(
    contract: &super::task_contract::TaskContract,
) -> Option<BehaviorContractProjection> {
    let rb = &contract.required_behavior;
    if !rb.confidence.is_finite() || rb.confidence < LOW_CONFIDENCE_THRESHOLD {
        return None;
    }
    let mut fields_used: Vec<&'static str> = Vec::with_capacity(FIELDS_USED_ORDER.len());
    if rb.behavior_goal.is_some() {
        fields_used.push("behavior_goal");
    }
    if rb
        .required_capabilities
        .as_ref()
        .is_some_and(|v| !v.is_empty())
    {
        fields_used.push("required_capabilities");
    }
    if rb
        .verification_expectations
        .as_ref()
        .is_some_and(|v| !v.is_empty())
    {
        fields_used.push("verification_expectations");
    }
    if rb.non_goals.as_ref().is_some_and(|v| !v.is_empty()) {
        fields_used.push("non_goals");
    }
    if fields_used.is_empty() {
        return None;
    }
    Some(BehaviorContractProjection {
        confidence: rb.confidence,
        fields_used,
        behavior_goal: rb.behavior_goal.clone(),
        required_capabilities: rb.required_capabilities.clone().unwrap_or_default(),
        verification_expectations: rb.verification_expectations.clone().unwrap_or_default(),
        non_goals: rb.non_goals.clone().unwrap_or_default(),
    })
}

/// Return true when the extracted behavior contract is strong enough to act as
/// verifier-repair authority.
///
/// `project_behavior_contract()` is intentionally broader: it can expose
/// domain terms, interface hints, setup labels, and verification labels as
/// diagnostic context. Those labels are useful for prompting, but they are too
/// weak to authorize exact-value implementation changes after a generated test
/// assertion fails. This predicate is the narrower boundary used by repair
/// authority selection.
pub(super) fn behavior_contract_has_repair_authority(
    contract: &super::task_contract::TaskContract,
) -> bool {
    let rb = &contract.required_behavior;
    if rb.behavior_goal.is_some() {
        return true;
    }
    if rb.non_goals.as_ref().is_some_and(|items| !items.is_empty()) {
        return true;
    }
    rb.operations.as_ref().is_some_and(|ops| {
        ops.iter().any(|op| {
            matches!(
                op,
                Operation::Create | Operation::Read | Operation::Update | Operation::Delete
            )
        })
    })
}

// ---------------------------------------------------------------------------
// Issue #664 (AD13 / AD18 / AD22 / Stage B fallback helpers).
//
// Label-substring helpers closed over the `BehaviorContractProjection`
// surface so the substring evaluation lives in this module rather than
// leaking into `active_job_arbiter.rs` / `task_contract.rs`. Both helpers
// are `pub(super)` and consumed solely from
// `active_job_arbiter::should_install_setup_bootstrap` (Setup label fallback)
// and `task_contract::VerifierPrerequisiteSignal::from_sources` (verifier
// capability Stage B fallback).
// ---------------------------------------------------------------------------

/// Set of substring needles classified as a `Setup` capability / expectation
/// label (旧 AD9 補助 fallback / AD13 で格下げ). Lowercased ASCII-only;
/// callers must lowercase before matching. The deterministic order is
/// pinned via the slice literal so future additions are review-visible.
const SETUP_LABEL_NEEDLES: &[&str] = &[
    "install",
    "setup",
    "bootstrap",
    "configure",
    "dependency",
    "environment",
];

/// Set of substring needles classified as a `Verifier prerequisite`
/// capability label (Stage B / 候補 b for AD18 verifier prerequisite signal).
const VERIFIER_CAPABILITY_NEEDLES: &[&str] = &["test", "verify"];

/// `true` iff the projection carries any `required_capabilities` /
/// `verification_expectations` label whose lower-cased form contains
/// one of [`SETUP_LABEL_NEEDLES`]. Used by `should_install_setup_bootstrap`
/// as the last-resort fallback when neither `required_artifacts::Setup`
/// nor `optional_artifacts::Setup` / verifier prerequisite signal fired.
pub(super) fn behavior_projection_has_setup_label(p: &BehaviorContractProjection) -> bool {
    label_set_hits_needle(&p.required_capabilities, SETUP_LABEL_NEEDLES)
        || label_set_hits_needle(&p.verification_expectations, SETUP_LABEL_NEEDLES)
}

/// `true` iff the projection carries any `required_capabilities` /
/// `verification_expectations` label whose lower-cased form contains
/// one of [`VERIFIER_CAPABILITY_NEEDLES`]. Used as Stage B of the
/// verifier prerequisite signal (AD18 / candidate (b)) — the `task_contract.rs`
/// `VerifierPrerequisiteSignal::from_sources` constructor folds this in
/// when caller has no live `OwnedTestVerifierPlan::Missing` observation.
pub(super) fn behavior_projection_has_verifier_capability(p: &BehaviorContractProjection) -> bool {
    label_set_hits_needle(&p.required_capabilities, VERIFIER_CAPABILITY_NEEDLES)
        || label_set_hits_needle(&p.verification_expectations, VERIFIER_CAPABILITY_NEEDLES)
}

/// Pure helper: returns true iff any `BoundedLabelWithExcerpt::label` in
/// `entries`, lower-cased, contains any string in `needles`.
fn label_set_hits_needle(entries: &[BoundedLabelWithExcerpt], needles: &[&str]) -> bool {
    entries.iter().any(|entry| {
        let lower = entry.label.to_ascii_lowercase();
        needles.iter().any(|needle| lower.contains(needle))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Operation {
    Create,
    Read,
    Update,
    Delete,
    Run,
    Validate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VerificationKind {
    Test,
    Build,
    Run,
    Smoke,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArtifactKind {
    Implementation,
    Test,
    UsageDocs,
    Setup,
}

impl ArtifactKind {
    /// Map behavior-schema `ArtifactKind` to the agent-loop `ArtifactRole`
    /// SSOT used by existing completion gates.
    pub(super) fn to_role(self) -> ArtifactRole {
        match self {
            ArtifactKind::Implementation => ArtifactRole::Implementation,
            ArtifactKind::Test => ArtifactRole::Test,
            ArtifactKind::UsageDocs => ArtifactRole::UsageDocs,
            ArtifactKind::Setup => ArtifactRole::Setup,
        }
    }
}

/// Closed allowlist of interface hints (DR1-005). Keeping this as an enum
/// means extractor / validator / tests only need to add a new variant when
/// the allowlist grows — there's no free-form `String` interface hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InterfaceHint {
    Api,
    Cli,
    WebUi,
    Library,
}

/// Schema validation failure. Hand-rolled `Display` / `Error` impls keep
/// `Cargo.toml` untouched (no `thiserror` direct dependency exists today,
/// DR2-007 / DR3-001).
#[derive(Debug, PartialEq)]
pub(super) enum SchemaError {
    /// One of the `Option<Vec<_>>` fields exceeded [`MAX_ARRAY`].
    ArrayTooLong { field: &'static str, len: usize },
    /// A `domain_terms` entry exceeded [`MAX_STR`] bytes.
    DomainTermTooLong { len: usize },
    /// `confidence` was non-finite or outside `[0.0, 1.0]`.
    ConfidenceInvalid { value: f32 },
    /// Issue #665: A [`BoundedLabelWithExcerpt::label`] entry on one of the
    /// new fields exceeded [`LABEL_MAX_LEN`] bytes. `field` SSOT values:
    /// `"behavior_goal"` / `"required_capabilities"` /
    /// `"verification_expectations"` / `"non_goals"`.
    LabelTooLong { field: &'static str, len: usize },
    /// Issue #665: A [`BoundedLabelWithExcerpt::excerpt`] entry exceeded
    /// [`EXCERPT_MAX_LEN`] bytes. `field` uses the same SSOT values as
    /// [`SchemaError::LabelTooLong`].
    ExcerptTooLong { field: &'static str, len: usize },
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaError::ArrayTooLong { field, len } => {
                write!(f, "{field} array too long: {len} > {MAX_ARRAY}")
            }
            SchemaError::DomainTermTooLong { len } => {
                write!(f, "domain_term too long: {len} > {MAX_STR} bytes")
            }
            SchemaError::ConfidenceInvalid { value } => {
                write!(
                    f,
                    "confidence invalid (must be finite in [0.0, 1.0]): {value}"
                )
            }
            SchemaError::LabelTooLong { field, len } => {
                write!(f, "{field} label too long: {len} > {LABEL_MAX_LEN} bytes")
            }
            SchemaError::ExcerptTooLong { field, len } => {
                write!(
                    f,
                    "{field} excerpt too long: {len} > {EXCERPT_MAX_LEN} bytes"
                )
            }
        }
    }
}

impl std::error::Error for SchemaError {}

/// Helper that returns `ArrayTooLong` when `items` exceeds [`MAX_ARRAY`].
fn check_array_len<T>(field: &'static str, items: Option<&[T]>) -> Result<(), SchemaError> {
    if let Some(items) = items
        && items.len() > MAX_ARRAY
    {
        return Err(SchemaError::ArrayTooLong {
            field,
            len: items.len(),
        });
    }
    Ok(())
}

/// Issue #665: Validate a single [`BoundedLabelWithExcerpt`]'s `label` and
/// optional `excerpt` lengths. `field` becomes the SSOT-aligned source name
/// surfaced by [`SchemaError::LabelTooLong`] / [`SchemaError::ExcerptTooLong`].
fn validate_label_with_excerpt(
    field: &'static str,
    item: &BoundedLabelWithExcerpt,
) -> Result<(), SchemaError> {
    if item.label.len() > LABEL_MAX_LEN {
        return Err(SchemaError::LabelTooLong {
            field,
            len: item.label.len(),
        });
    }
    if let Some(excerpt) = item.excerpt.as_ref()
        && excerpt.len() > EXCERPT_MAX_LEN
    {
        return Err(SchemaError::ExcerptTooLong {
            field,
            len: excerpt.len(),
        });
    }
    Ok(())
}

/// Issue #665: Validate an `Option<Vec<BoundedLabelWithExcerpt>>` field
/// (array length + per-entry label / excerpt).
fn validate_bounded_label_vec(
    field: &'static str,
    items: Option<&[BoundedLabelWithExcerpt]>,
) -> Result<(), SchemaError> {
    check_array_len(field, items)?;
    if let Some(items) = items {
        for item in items {
            validate_label_with_excerpt(field, item)?;
        }
    }
    Ok(())
}

impl RequiredBehaviorContract {
    /// Validate that the contract obeys the closed schema invariants
    /// (`MAX_ARRAY` / `MAX_STR` / finite confidence in `[0.0, 1.0]`).
    ///
    /// In Issue #635 scope the extractor is the only producer and the
    /// caller is `TaskContract::from_request`, which constructs values
    /// with `extract()` (guaranteed to validate). The method is therefore
    /// invoked through `debug_assert!(...validate().is_ok())` at the
    /// construction site. Once a JSON / LLM input path lands, callers
    /// should switch to propagating the `Result`.
    pub(super) fn validate(&self) -> Result<(), SchemaError> {
        if !self.confidence.is_finite() || !(0.0..=1.0).contains(&self.confidence) {
            return Err(SchemaError::ConfidenceInvalid {
                value: self.confidence,
            });
        }
        check_array_len("operations", self.operations.as_deref())?;
        if let Some(terms) = self.domain_terms.as_deref() {
            check_array_len("domain_terms", Some(terms))?;
            for term in terms {
                if term.len() > MAX_STR {
                    return Err(SchemaError::DomainTermTooLong { len: term.len() });
                }
            }
        }
        check_array_len("interface_hints", self.interface_hints.as_deref())?;
        check_array_len("required_artifacts", self.required_artifacts.as_deref())?;
        check_array_len("verification", self.verification.as_deref())?;
        // Issue #665: validate the 4 new fields. `behavior_goal` is singular
        // so it goes through the per-entry helper directly; the other three
        // pass through `check_array_len` first.
        if let Some(goal) = self.behavior_goal.as_ref() {
            validate_label_with_excerpt("behavior_goal", goal)?;
        }
        validate_bounded_label_vec(
            "required_capabilities",
            self.required_capabilities.as_deref(),
        )?;
        validate_bounded_label_vec(
            "verification_expectations",
            self.verification_expectations.as_deref(),
        )?;
        validate_bounded_label_vec("non_goals", self.non_goals.as_deref())?;
        Ok(())
    }

    /// Convert behavior schema's `required_artifacts` to
    /// [`ArtifactRole`] (the SSOT used by completion / recovery code).
    ///
    /// Returns an empty `Vec` when `required_artifacts` is `None`
    /// (unknown). In Issue #635 this getter is intentionally **not**
    /// called by `TaskContract::from_request` — the existing
    /// `request_asks_for_*` gates remain the source of truth for the
    /// `required_artifacts` field. Issue #636 will become the sole
    /// caller when it switches the contract to a derived-value path
    /// (design policy §7 #1).
    #[allow(dead_code)]
    pub(super) fn to_artifact_roles(&self) -> Vec<ArtifactRole> {
        let Some(kinds) = &self.required_artifacts else {
            return Vec::new();
        };
        let mut roles: Vec<ArtifactRole> =
            kinds.iter().copied().map(ArtifactKind::to_role).collect();
        roles.sort();
        roles.dedup();
        roles
    }

    /// Issue #636: judgement API — does `excerpt` hit any of the
    /// operation keywords backing `self.operations`?
    ///
    /// Short ASCII keywords (`read`, `run`) go through the token-boundary
    /// `keyword_hit` SSOT so `README` / `running` do not false-positive.
    /// `KeywordMatch` / `OPERATION_KEYWORDS` stay private to this module
    /// (DR1-005 / DR3-001 — no facade re-export). Returns `false` when
    /// `operations` is `None` or empty.
    pub(super) fn excerpt_hits_any_operation(&self, excerpt: &str) -> bool {
        let Some(ops) = self.operations.as_ref() else {
            return false;
        };
        if ops.is_empty() {
            return false;
        }
        let lower = excerpt.to_ascii_lowercase();
        for (needle, op, mode) in OPERATION_KEYWORDS {
            if ops.contains(op) && keyword_hit(&lower, needle, *mode) {
                return true;
            }
        }
        false
    }

    /// Issue #652: judgement API — does this contract require a test artifact
    /// to be exercised by the verifier?
    ///
    /// After Issue #651 landed the explicit `test_execution_required` boolean
    /// (SSOT-aligned with `super::task_contract::request_asks_for_test_artifact`),
    /// this helper just reads that field directly (DR1-007 / design judgement
    /// #3). The call-site contract is unchanged; callers in `turn.rs` keep
    /// using `requires_test_execution()` and the field stays a single source
    /// of truth.
    #[allow(dead_code)] // wired through unit tests today; #651 will land the explicit call site.
    pub(super) fn requires_test_execution(&self) -> bool {
        self.test_execution_required
    }

    /// Issue #636: judgement API — does `excerpt` hit any of the
    /// `domain_terms`?
    ///
    /// `domain_terms` are user-derived vocabulary so we match by
    /// case-insensitive substring without applying the short-token
    /// boundary rule (CB-004 keeps schema vs request alignment). Returns
    /// `false` when `domain_terms` is `None` or empty.
    pub(super) fn excerpt_hits_any_domain_term(&self, excerpt: &str) -> bool {
        let Some(terms) = self.domain_terms.as_ref() else {
            return false;
        };
        if terms.is_empty() {
            return false;
        }
        let lower = excerpt.to_ascii_lowercase();
        terms
            .iter()
            .any(|term| domain_term_matches_excerpt(term, &lower))
    }
}

fn domain_term_matches_excerpt(term: &str, lower_excerpt: &str) -> bool {
    if term.is_empty() {
        return false;
    }
    let lower_term = term.to_ascii_lowercase();
    if lower_excerpt.contains(&lower_term) {
        return true;
    }
    domain_term_tail(&lower_term)
        .is_some_and(|tail| tail.len() >= 3 && lower_excerpt.contains(tail))
}

fn domain_term_tail(term: &str) -> Option<&str> {
    term.rsplit(|ch| matches!(ch, '.' | '/' | '-' | '_'))
        .find(|part| !part.is_empty())
}

// ---------------------------------------------------------------------------
// Stage 1: candidate generation (`extract`).
// ---------------------------------------------------------------------------

/// Build a [`RequiredBehaviorContract`] candidate from `request` and then
/// pin it down with [`filter_against_request`].
///
/// Pure function: no LLM call, no I/O, no `Agent` / `SessionSnapshot`
/// access. Long inputs are truncated to `MAX_REQUEST_SCAN_BYTES` UTF-8
/// bytes (char boundary preserved) and run through
/// [`mask_secrets`] before any field is built.
pub(super) fn extract(request: &str) -> RequiredBehaviorContract {
    let scan_text = bounded_masked_request(request);
    let scan = scan_text.as_str();
    let lower = scan.to_ascii_lowercase();
    // Issue #651: SSOT predicate is computed once and reused by both
    // `extract_required_artifacts` (via `asks_for_tests` inside that
    // helper) and the `test_execution_required` field below.
    // `request_asks_for_test_artifact` is invoked here exactly once; the
    // call inside `extract_required_artifacts` reads the same masked
    // scan so the two stay in lock-step.
    let asks_for_tests = super::task_contract::request_asks_for_test_artifact(scan, &lower);
    let operations = extract_operations(scan, &lower);
    let domain_terms = extract_domain_terms(scan);
    let interface_hints = extract_interface_hints(&lower);
    let required_artifacts = extract_required_artifacts(request, scan, &lower);
    let verification = extract_verification(&lower);
    let confidence = confidence_from_hits(
        operations.as_ref(),
        domain_terms.as_ref(),
        interface_hints.as_ref(),
        required_artifacts.as_ref(),
        verification.as_ref(),
    );
    // Issue #665 — Phase 3: 4 new fields.
    // - behavior_goal / non_goals: 専用 helper で raw excerpt と共に抽出。
    // - required_capabilities / verification_expectations: 既存 fields の
    //   projection として derive（excerpt: None 原則 / S5-003）。
    let behavior_goal = extract_behavior_goal(scan);
    let non_goals = extract_non_goals(scan);
    let required_capabilities =
        derive_required_capabilities(operations.as_deref(), domain_terms.as_deref());
    let verification_expectations = derive_verification_expectations(verification.as_deref());
    let candidate = RequiredBehaviorContract {
        operations,
        domain_terms,
        interface_hints,
        required_artifacts,
        verification,
        confidence,
        test_execution_required: asks_for_tests,
        behavior_goal,
        required_capabilities,
        verification_expectations,
        non_goals,
    };
    debug_assert!(
        candidate.validate().is_ok(),
        "extract produced invalid schema"
    );
    // Stage 2: pin the candidate against the request text. In Issue #635
    // scope the inputs are the same so this is essentially a no-op, but
    // pinning the call here keeps the security contract symmetrical:
    // every value that leaves this module is request-backed and cap-safe.
    filter_against_request(&candidate, request)
}

/// Truncate `request` to `MAX_REQUEST_SCAN_BYTES` on a char boundary and
/// pass it through the session-layer redactor SSOT.
///
/// The redactor stack mirrors `redact_verifier_command_for_storage`
/// (DR4-002): first [`mask_secrets`] handles token / kv / URL redaction,
/// then [`mask_header_family`] strips `Authorization` / `Cookie` /
/// `X-API-Key` header credentials even when their tail does not match
/// the `kv_secret_regex` keyword set. Without the second pass a short
/// header credential (< MAX_STR bytes) inside a backtick span could
/// reach `domain_terms` verbatim — see CB-002.
fn bounded_masked_request(request: &str) -> String {
    let mut end = request.len().min(MAX_REQUEST_SCAN_BYTES);
    while end > 0 && !request.is_char_boundary(end) {
        end -= 1;
    }
    let s1 = crate::session::feedback::mask_secrets(&request[..end]);
    crate::session::feedback::mask_header_family(&s1)
}

/// `Operation` keyword table. ASCII short words (`read`, `run`) need
/// token-boundary checking; longer words can use plain substring matching.
const OPERATION_KEYWORDS: &[(&str, Operation, KeywordMatch)] = &[
    ("create", Operation::Create, KeywordMatch::Substring),
    ("read", Operation::Read, KeywordMatch::TokenAscii),
    ("update", Operation::Update, KeywordMatch::Substring),
    ("delete", Operation::Delete, KeywordMatch::Substring),
    ("run", Operation::Run, KeywordMatch::TokenAscii),
    ("validate", Operation::Validate, KeywordMatch::Substring),
];

/// `VerificationKind` keyword table.
const VERIFICATION_KEYWORDS: &[(&str, VerificationKind, KeywordMatch)] = &[
    ("test", VerificationKind::Test, KeywordMatch::Substring),
    ("build", VerificationKind::Build, KeywordMatch::Substring),
    ("run", VerificationKind::Run, KeywordMatch::TokenAscii),
    ("smoke", VerificationKind::Smoke, KeywordMatch::Substring),
];

/// `InterfaceHint` keyword table (closed enum, snake_case form).
const INTERFACE_HINT_KEYWORDS: &[(&str, InterfaceHint, KeywordMatch)] = &[
    ("api", InterfaceHint::Api, KeywordMatch::TokenAscii),
    ("cli", InterfaceHint::Cli, KeywordMatch::TokenAscii),
    ("web_ui", InterfaceHint::WebUi, KeywordMatch::Substring),
    ("webui", InterfaceHint::WebUi, KeywordMatch::Substring),
    ("library", InterfaceHint::Library, KeywordMatch::Substring),
];

#[derive(Clone, Copy)]
enum KeywordMatch {
    /// `haystack.contains(needle)` is good enough (the keyword is long
    /// enough not to clash with English words like `read` in `README`).
    Substring,
    /// `read` / `run` style short ASCII words need a token boundary
    /// check so `README` / `running` don't false-positive.
    TokenAscii,
}

fn keyword_hit(lower: &str, needle: &str, mode: KeywordMatch) -> bool {
    match mode {
        KeywordMatch::Substring => lower.contains(needle),
        KeywordMatch::TokenAscii => contains_ascii_token(lower, needle),
    }
}

fn extract_operations(_scan: &str, lower: &str) -> Option<Vec<Operation>> {
    let mut hits: Vec<Operation> = Vec::new();
    for (needle, op, mode) in OPERATION_KEYWORDS {
        if keyword_hit(lower, needle, *mode) && !hits.contains(op) {
            hits.push(*op);
        }
    }
    if lower.contains("crud") {
        for op in [
            Operation::Create,
            Operation::Read,
            Operation::Update,
            Operation::Delete,
        ] {
            if !hits.contains(&op) {
                hits.push(op);
            }
        }
    }
    if hits.is_empty() {
        None
    } else {
        hits.truncate(MAX_ARRAY);
        Some(hits)
    }
}

fn extract_verification(lower: &str) -> Option<Vec<VerificationKind>> {
    let mut hits: Vec<VerificationKind> = Vec::new();
    for (needle, kind, mode) in VERIFICATION_KEYWORDS {
        if keyword_hit(lower, needle, *mode) && !hits.contains(kind) {
            hits.push(*kind);
        }
    }
    if hits.is_empty() {
        None
    } else {
        hits.truncate(MAX_ARRAY);
        Some(hits)
    }
}

fn extract_interface_hints(lower: &str) -> Option<Vec<InterfaceHint>> {
    let mut hits: Vec<InterfaceHint> = Vec::new();
    for (needle, hint, mode) in INTERFACE_HINT_KEYWORDS {
        if keyword_hit(lower, needle, *mode) && !hits.contains(hint) {
            hits.push(*hint);
        }
    }
    if hits.is_empty() {
        None
    } else {
        hits.truncate(MAX_ARRAY);
        Some(hits)
    }
}

/// Pull out "code-like" tokens from the bounded scan: backtick / quoted
/// spans, paths, CamelCase / snake_case identifiers. Each surviving term
/// must round-trip through [`mask_secrets`] unchanged and must not contain
/// any redaction sentinel, so raw secret-like values never reach the
/// schema (DR4-001). At most [`MAX_ARRAY`] terms are returned and each is
/// at most [`MAX_STR`] bytes.
fn extract_domain_terms(scan: &str) -> Option<Vec<String>> {
    let mut hits: Vec<String> = Vec::new();
    collect_delimited_terms(scan, '`', '`', &mut hits);
    collect_delimited_terms(scan, '"', '"', &mut hits);
    collect_delimited_terms(scan, '\'', '\'', &mut hits);
    if hits.len() < MAX_ARRAY {
        collect_identifier_terms(scan, &mut hits);
    }
    sanitize_domain_terms(hits)
}

/// Iterate `scan` once, pulling out `open ... close` spans in linear
/// order. Uses an absolute `cursor: usize` byte offset into `scan` so
/// that consecutive spans (`` `Foo` and `Bar` ``) all advance forward
/// without resetting (CB-001). The previous implementation rebuilt a
/// `char_indices()` iterator on the suffix but ignored the suffix
/// origin, which could either revisit the same delimiter or loop
/// forever when the same delimiter character appeared twice in
/// short succession.
fn collect_delimited_terms(scan: &str, open: char, close: char, hits: &mut Vec<String>) {
    let mut cursor: usize = 0;
    while cursor < scan.len() {
        let Some(rel_open) = scan[cursor..].find(open) else {
            return;
        };
        let open_byte = cursor + rel_open;
        let term_start = open_byte + open.len_utf8();
        if term_start > scan.len() {
            return;
        }
        let Some(rel_close) = scan[term_start..].find(close) else {
            return;
        };
        let close_byte = term_start + rel_close;
        let term = &scan[term_start..close_byte];
        if !term.is_empty() {
            push_domain_term(hits, term);
            if hits.len() >= MAX_ARRAY {
                return;
            }
        }
        // Always advance past the closing delimiter so the next iteration
        // makes forward progress. Even if `term` was empty / rejected we
        // must not revisit `close_byte`.
        cursor = close_byte + close.len_utf8();
    }
}

fn collect_identifier_terms(scan: &str, hits: &mut Vec<String>) {
    let mut current = String::new();
    for ch in scan.chars() {
        if is_identifier_char(ch) {
            current.push(ch);
        } else {
            consider_identifier(&current, hits);
            current.clear();
            if hits.len() >= MAX_ARRAY {
                return;
            }
        }
    }
    consider_identifier(&current, hits);
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '/' || ch == '-'
}

fn consider_identifier(candidate: &str, hits: &mut Vec<String>) {
    if hits.len() >= MAX_ARRAY {
        return;
    }
    if candidate.is_empty() {
        return;
    }
    if !looks_like_domain_term(candidate) {
        return;
    }
    push_domain_term(hits, candidate);
}

fn looks_like_domain_term(candidate: &str) -> bool {
    // Skip pure-ASCII lowercase common-word tokens; we want identifiers
    // that look "code-like": CamelCase, snake_case, paths, dotted names,
    // hyphenated names, or anything with a digit.
    let has_upper = candidate.chars().any(|c| c.is_ascii_uppercase());
    let has_struct_punct =
        candidate.contains('_') || candidate.contains('.') || candidate.contains('/');
    let has_digit = candidate.chars().any(|c| c.is_ascii_digit());
    has_upper || has_struct_punct || has_digit
}

fn push_domain_term(hits: &mut Vec<String>, term: &str) {
    if hits.len() >= MAX_ARRAY {
        return;
    }
    let trimmed = term.trim();
    if trimmed.is_empty() {
        return;
    }
    if trimmed.len() > MAX_STR {
        return;
    }
    let owned = trimmed.to_string();
    if hits.iter().any(|existing| existing == &owned) {
        return;
    }
    hits.push(owned);
}

/// Drop any term that `mask_secrets` would change or that contains a
/// redaction sentinel, so raw secret-like tokens never reach the schema.
fn sanitize_domain_terms(hits: Vec<String>) -> Option<Vec<String>> {
    let mut clean: Vec<String> = Vec::new();
    for term in hits {
        if term.len() > MAX_STR {
            continue;
        }
        if contains_redaction_sentinel(&term) {
            continue;
        }
        if crate::session::feedback::mask_secrets(&term) != term {
            continue;
        }
        clean.push(term);
        if clean.len() >= MAX_ARRAY {
            break;
        }
    }
    if clean.is_empty() { None } else { Some(clean) }
}

fn contains_redaction_sentinel(term: &str) -> bool {
    REDACTION_SENTINELS.iter().any(|s| term.contains(s))
}

/// Thin delegation to the existing `request_asks_for_*` gates in
/// `task_contract.rs` (DR1-004). The artifact gate is the canonical
/// source of truth; we just project it into closed-enum form so the
/// schema stays self-contained.
///
/// ## Inputs
///
/// - `_request` is currently unused; the bounded masked `scan` /
///   `lower` views are the only source of truth (CB-003). The argument
///   is retained for symmetry with [`filter_against_request`], which
///   needs the raw request for its own redaction pass.
/// - `scan` is the [`bounded_masked_request`] result.
/// - `lower` is `scan.to_ascii_lowercase()`.
///
/// ## CB-003 (bounded scan)
///
/// Both raw and lowercase inputs to the gates are sourced from the
/// bounded masked scan — never the unbounded raw `request`. This keeps
/// the schema honest for >64KiB requests where a trigger token past
/// the cap would otherwise still flip an artifact bit.
///
/// ## CB-004 (Install intent alignment)
///
/// Setup follows the **same** `Install`-only rule as
/// [`task_contract::TaskContract::from_request`]. The rule is
/// canonically defined by
/// [`task_contract::request_asks_for_code_work`]:
///
/// ```text
/// Install ⇔ asks_for_setup && !request_asks_for_code_work
/// ```
///
/// We reuse `request_asks_for_code_work` verbatim instead of
/// approximating it with `!asks_for_impl`. The approximation was
/// unsound for requests like `"add dependencies to package.json"`
/// where:
///
/// - support-aware `asks_for_impl` is `false` (only `add` matches and
///   the support branch needs a `production_action`), but
/// - support-blind `asks_for_code_work` is `true` (the unconstrained
///   `edit_action` branch picks up `add`).
///
/// Without the shared rule, the schema would have flagged Setup
/// `required` while `TaskContract` kept it `optional`.
fn extract_required_artifacts(
    _request: &str,
    scan: &str,
    lower: &str,
) -> Option<Vec<ArtifactKind>> {
    let asks_for_tests = super::task_contract::request_asks_for_test_artifact(scan, lower);
    let asks_for_usage_docs = super::task_contract::request_asks_for_usage_docs(scan, lower);
    let asks_for_setup = super::task_contract::request_asks_for_setup(scan, lower);
    let asks_for_impl = super::task_contract::request_asks_for_implementation_artifact(
        scan,
        lower,
        asks_for_tests,
        asks_for_usage_docs,
        asks_for_setup,
    );

    let mut kinds: Vec<ArtifactKind> = Vec::new();
    if asks_for_impl {
        kinds.push(ArtifactKind::Implementation);
    }
    if asks_for_tests {
        kinds.push(ArtifactKind::Test);
    }
    if asks_for_usage_docs {
        kinds.push(ArtifactKind::UsageDocs);
    }
    if asks_for_setup && setup_is_required(scan, lower) {
        kinds.push(ArtifactKind::Setup);
    }
    if kinds.is_empty() { None } else { Some(kinds) }
}

/// Mirror [`task_contract::TaskContract::from_request`]'s rule for
/// promoting Setup from `optional` to `required`: Setup is only
/// required when intent is `Install`, which
/// [`task_contract::infer_intent`] (the canonical site) defines as
/// `asks_for_setup && !request_asks_for_code_work` (CB-004).
///
/// This calls
/// [`task_contract::request_asks_for_code_work`] directly so the two
/// paths can never drift. The caller is responsible for the leading
/// `asks_for_setup` check.
fn setup_is_required(scan: &str, lower: &str) -> bool {
    !super::task_contract::request_asks_for_code_work(scan, lower)
}

fn confidence_from_hits(
    operations: Option<&Vec<Operation>>,
    domain_terms: Option<&Vec<String>>,
    interface_hints: Option<&Vec<InterfaceHint>>,
    required_artifacts: Option<&Vec<ArtifactKind>>,
    verification: Option<&Vec<VerificationKind>>,
) -> f32 {
    let any_hit = operations.map(|v| !v.is_empty()).unwrap_or(false)
        || domain_terms.map(|v| !v.is_empty()).unwrap_or(false)
        || interface_hints.map(|v| !v.is_empty()).unwrap_or(false)
        || required_artifacts.map(|v| !v.is_empty()).unwrap_or(false)
        || verification.map(|v| !v.is_empty()).unwrap_or(false);
    if any_hit { 1.0 } else { 0.0 }
}

/// Token-boundary `contains` for ASCII short words. Mirrors the helper
/// already used in `task_contract.rs`.
fn contains_ascii_token(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(idx, _)| {
        let before = haystack[..idx]
            .chars()
            .next_back()
            .is_none_or(|ch| !ch.is_ascii_alphanumeric());
        let after_idx = idx + needle.len();
        let after = haystack[after_idx..]
            .chars()
            .next()
            .is_none_or(|ch| !ch.is_ascii_alphanumeric());
        before && after
    })
}

// ---------------------------------------------------------------------------
// Stage 2: request-backed filter (`filter_against_request`).
// ---------------------------------------------------------------------------

/// Pin a candidate [`RequiredBehaviorContract`] against `request` text.
///
/// Algorithm:
///
/// - `operations`: keep variants whose snake_case keyword appears in the
///   bounded masked lowercase scan (token-boundary checked for short
///   ASCII words).
/// - `domain_terms`: each candidate term is treated as untrusted —
///   reapply `mask_secrets`, drop redaction sentinels and oversize
///   terms, then keep only those that appear in the bounded scan.
/// - `interface_hints` / `verification`: same keyword check as
///   `operations`.
/// - `required_artifacts`: closed-enum gate output, kept as-is (already
///   request-backed via the deterministic `request_asks_for_*`
///   delegation).
/// - `confidence`: recomputed from the surviving fields; the candidate
///   value is **not** trusted.
///
/// The function re-applies [`bounded_masked_request`] internally, so it
/// is safe to call with the original raw `request` even if the candidate
/// was built from a different source (e.g. a future LLM proposal).
pub(super) fn filter_against_request(
    candidate: &RequiredBehaviorContract,
    request: &str,
) -> RequiredBehaviorContract {
    let scan_text = bounded_masked_request(request);
    let scan = scan_text.as_str();
    let lower = scan.to_ascii_lowercase();

    let operations = candidate.operations.as_ref().and_then(|ops| {
        let mut kept: Vec<Operation> = Vec::new();
        for op in ops {
            if kept.contains(op) {
                continue;
            }
            if !operation_in_request(*op, &lower) {
                continue;
            }
            kept.push(*op);
            if kept.len() >= MAX_ARRAY {
                break;
            }
        }
        if kept.is_empty() { None } else { Some(kept) }
    });

    let domain_terms = candidate
        .domain_terms
        .as_ref()
        .and_then(|terms| filter_domain_terms(terms, scan));

    let interface_hints = candidate.interface_hints.as_ref().and_then(|hints| {
        let mut kept: Vec<InterfaceHint> = Vec::new();
        for hint in hints {
            if kept.contains(hint) {
                continue;
            }
            if !interface_hint_in_request(*hint, &lower) {
                continue;
            }
            kept.push(*hint);
            if kept.len() >= MAX_ARRAY {
                break;
            }
        }
        if kept.is_empty() { None } else { Some(kept) }
    });

    // CB-005: candidate.required_artifacts is treated as untrusted. We
    // recompute the request-backed artifact set against the bounded
    // masked scan (same source as the rest of the filter) and intersect
    // with the candidate. Anything the request does not back is
    // dropped, so e.g. a candidate Test against an "explain Foo"
    // request becomes empty.
    let backed_artifacts = extract_required_artifacts(request, scan, &lower);
    let required_artifacts = candidate.required_artifacts.as_ref().map(|arts| {
        let mut kept: Vec<ArtifactKind> = Vec::new();
        for art in arts {
            if kept.contains(art) {
                continue;
            }
            let backed = backed_artifacts.as_ref().is_some_and(|b| b.contains(art));
            if !backed {
                continue;
            }
            kept.push(*art);
            if kept.len() >= MAX_ARRAY {
                break;
            }
        }
        kept
    });
    let required_artifacts = required_artifacts.filter(|v| !v.is_empty());

    let verification = candidate.verification.as_ref().and_then(|kinds| {
        let mut kept: Vec<VerificationKind> = Vec::new();
        for kind in kinds {
            if kept.contains(kind) {
                continue;
            }
            if !verification_in_request(*kind, &lower) {
                continue;
            }
            kept.push(*kind);
            if kept.len() >= MAX_ARRAY {
                break;
            }
        }
        if kept.is_empty() { None } else { Some(kept) }
    });

    let confidence = confidence_from_hits(
        operations.as_ref(),
        domain_terms.as_ref(),
        interface_hints.as_ref(),
        required_artifacts.as_ref(),
        verification.as_ref(),
    );

    // Issue #651: `verification` may have lost the Test variant during
    // narrowing above; the test_execution_required gate is still derived
    // from the bounded masked request directly so a `spec` / `テストも実装`
    // request keeps `test_execution_required = true` even when the
    // `Test` verification kind drops out (design judgement #1).
    let test_execution_required =
        super::task_contract::request_asks_for_test_artifact(scan, &lower);

    // Issue #665 — Phase 3 / S5-002 canonicality:
    // - behavior_goal / non_goals: candidate を信頼せず、raw request から
    //   再抽出（masked scan の出力に対して helper を再実行）。
    // - required_capabilities / verification_expectations: post-filter 後の
    //   source fields (`operations` / `domain_terms` / `verification`) から
    //   必ず derive（candidate の derived value はドリフト源になるので捨てる）。
    let scan_text = bounded_masked_request(request);
    let scan = scan_text.as_str();
    let behavior_goal = extract_behavior_goal(scan);
    let non_goals = extract_non_goals(scan);
    let required_capabilities =
        derive_required_capabilities(operations.as_deref(), domain_terms.as_deref());
    let verification_expectations = derive_verification_expectations(verification.as_deref());
    let filtered = RequiredBehaviorContract {
        operations,
        domain_terms,
        interface_hints,
        required_artifacts,
        verification,
        confidence,
        test_execution_required,
        behavior_goal,
        required_capabilities,
        verification_expectations,
        non_goals,
    };
    debug_assert!(
        filtered.validate().is_ok(),
        "filter_against_request produced invalid schema"
    );
    filtered
}

// ---------------------------------------------------------------------------
// Issue #665 Phase 3: behavior_goal / non_goals 抽出 helper
// ---------------------------------------------------------------------------

/// Issue #665 — Task 3.1: 1 行サマリ（user 要求の imperative 主文）を抽出する。
///
/// `scan` は既に [`bounded_masked_request`] (`mask_secrets` +
/// `mask_header_family`) を通過しているため、ここでの追加処理は:
/// 1. 改行 / 制御文字を空白に正規化
/// 2. 否定文 (do not / don't / disable / skip / never / は対象外 / しない) を
///    除外
/// 3. 最大 `LABEL_MAX_LEN` byte の bounded label と最大 `EXCERPT_MAX_LEN`
///    byte の excerpt を切り出し
///
/// Pure fn / no I/O.
fn extract_behavior_goal(scan: &str) -> Option<BoundedLabelWithExcerpt> {
    let normalized = normalize_control_chars(scan);
    if let Some(goal) = extract_first_behavior_goal_sentence(&normalized, looks_like_spec_section) {
        return Some(goal);
    }
    extract_first_behavior_goal_sentence(&normalized, |trimmed| {
        let lower = trimmed.to_ascii_lowercase();
        contains_imperative_verb(&lower)
    })
}

fn extract_first_behavior_goal_sentence(
    normalized: &str,
    predicate: impl Fn(&str) -> bool,
) -> Option<BoundedLabelWithExcerpt> {
    for sentence in split_into_sentences(&normalized) {
        let trimmed = sentence.trim();
        if trimmed.is_empty() || looks_like_negation(trimmed) {
            continue;
        }
        if !predicate(trimmed) {
            continue;
        }
        if contains_redaction_sentinel(trimmed) {
            continue;
        }
        let label = truncate_to_label(trimmed);
        if label.is_empty() {
            continue;
        }
        let excerpt = truncate_excerpt(trimmed);
        return Some(BoundedLabelWithExcerpt {
            label,
            excerpt: Some(excerpt),
        });
    }
    None
}

fn looks_like_spec_section(sentence: &str) -> bool {
    let lower = sentence.to_ascii_lowercase();
    const STRONG_SPEC_MARKERS: &[&str] = &[
        "contract:",
        "spec:",
        "specification:",
        "requirements:",
        "要件:",
        "仕様:",
        "仕様は",
    ];
    STRONG_SPEC_MARKERS
        .iter()
        .any(|marker| lower.contains(marker) || sentence.contains(marker))
}

/// Issue #665 — Task 3.2: non-goal 文（do not / don't / は対象外 / しない / 不要 /
/// 禁止）から bounded label + excerpt のリストを抽出する。
///
/// 最大 `MAX_ARRAY` 件、各要素は `LABEL_MAX_LEN` / `EXCERPT_MAX_LEN` で cap。
/// Pure fn / no I/O.
fn extract_non_goals(scan: &str) -> Option<Vec<BoundedLabelWithExcerpt>> {
    let normalized = normalize_control_chars(scan);
    let mut out: Vec<BoundedLabelWithExcerpt> = Vec::new();
    for sentence in split_into_sentences(&normalized) {
        let trimmed = sentence.trim();
        if trimmed.is_empty() || !looks_like_negation(trimmed) {
            continue;
        }
        if contains_redaction_sentinel(trimmed) {
            continue;
        }
        let label = truncate_to_label(trimmed);
        if label.is_empty() {
            continue;
        }
        let excerpt = truncate_excerpt(trimmed);
        let item = BoundedLabelWithExcerpt {
            label,
            excerpt: Some(excerpt),
        };
        if !out.iter().any(|existing| existing == &item) {
            out.push(item);
        }
        if out.len() >= MAX_ARRAY {
            break;
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Issue #665 — Task 3.3 / S5-003: post-filter `operations` + `domain_terms`
/// から bounded label のリストを derive する。`excerpt` は `None` 固定
/// (S5-003: derived field は raw excerpt を保持しない)。
fn derive_required_capabilities(
    operations: Option<&[Operation]>,
    domain_terms: Option<&[String]>,
) -> Option<Vec<BoundedLabelWithExcerpt>> {
    let mut out: Vec<BoundedLabelWithExcerpt> = Vec::new();
    if let Some(ops) = operations {
        for op in ops {
            let label = operation_label(*op);
            let item = BoundedLabelWithExcerpt {
                label: label.to_string(),
                excerpt: None,
            };
            if !out.iter().any(|e| e == &item) {
                out.push(item);
            }
            if out.len() >= MAX_ARRAY {
                break;
            }
        }
    }
    if let Some(terms) = domain_terms {
        for term in terms {
            if out.len() >= MAX_ARRAY {
                break;
            }
            let truncated = truncate_to_label(term);
            if truncated.is_empty() {
                continue;
            }
            let item = BoundedLabelWithExcerpt {
                label: truncated,
                excerpt: None,
            };
            if !out.iter().any(|e| e == &item) {
                out.push(item);
            }
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Issue #665 — Task 3.3 / S5-003: post-filter `verification` から bounded
/// label のリストを derive する。`excerpt` は `None` 固定。
fn derive_verification_expectations(
    verification: Option<&[VerificationKind]>,
) -> Option<Vec<BoundedLabelWithExcerpt>> {
    let verification = verification?;
    if verification.is_empty() {
        return None;
    }
    let mut out: Vec<BoundedLabelWithExcerpt> = Vec::new();
    for kind in verification {
        let label = verification_label(*kind);
        let item = BoundedLabelWithExcerpt {
            label: label.to_string(),
            excerpt: None,
        };
        if !out.iter().any(|e| e == &item) {
            out.push(item);
        }
        if out.len() >= MAX_ARRAY {
            break;
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// SSOT for projecting `Operation` enum → bounded label string.
fn operation_label(op: Operation) -> &'static str {
    match op {
        Operation::Create => "create",
        Operation::Read => "read",
        Operation::Update => "update",
        Operation::Delete => "delete",
        Operation::Run => "run",
        Operation::Validate => "validate",
    }
}

/// SSOT for projecting `VerificationKind` enum → bounded label string.
fn verification_label(kind: VerificationKind) -> &'static str {
    match kind {
        VerificationKind::Test => "test",
        VerificationKind::Build => "build",
        VerificationKind::Run => "run",
        VerificationKind::Smoke => "smoke",
    }
}

/// 制御文字 (\n / \t / \r 等) を空白に正規化。bounded_masked_request 通過後の
/// 文字列に対して呼ぶ。
fn normalize_control_chars(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() && c != ' ' { ' ' } else { c })
        .collect()
}

/// 文を ASCII 句読点 (`.`, `!`, `?`) と日本語句点 (`。`) で分割する単純な分割。
/// 完璧な構文解析ではなく bounded heuristic。
fn split_into_sentences(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'.' || c == b'!' || c == b'?' || c == b'\n' {
            // unsafe-free: ASCII boundary so safe
            if let Some(sub) = s.get(start..i) {
                out.push(sub);
            }
            start = i + 1;
            i += 1;
            continue;
        }
        // 日本語句点 (U+3002 = 0xE3 0x80 0x82) を検出
        if i + 3 <= bytes.len() && bytes[i] == 0xE3 && bytes[i + 1] == 0x80 && bytes[i + 2] == 0x82
        {
            if let Some(sub) = s.get(start..i) {
                out.push(sub);
            }
            start = i + 3;
            i += 3;
            continue;
        }
        i += 1;
    }
    if start < s.len()
        && let Some(sub) = s.get(start..)
    {
        out.push(sub);
    }
    out
}

/// 文が否定 / non-goal を表すかの heuristic。
fn looks_like_negation(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    const ENGLISH_NEGATIONS: &[&str] = &[
        "do not ",
        "don't ",
        "doesn't ",
        "should not ",
        "shouldn't ",
        "must not ",
        "mustn't ",
        "won't ",
        "no need ",
        "without ",
        "skip ",
        "disable ",
        "avoid ",
        "never ",
        "not allowed ",
        "out of scope",
    ];
    for n in ENGLISH_NEGATIONS {
        if lower.contains(n) {
            return true;
        }
    }
    // 日本語パターン（小文字化は ASCII のみ影響、日本語はそのまま）
    const JP_NEGATIONS: &[&str] = &[
        "しない",
        "は対象外",
        "対象外",
        "不要",
        "禁止",
        "は行わない",
        "は除外",
    ];
    for n in JP_NEGATIONS {
        if s.contains(n) {
            return true;
        }
    }
    false
}

/// imperative / 動作 keyword を含むか（behavior_goal の minimum filter）。
fn contains_imperative_verb(lower: &str) -> bool {
    // operation keywords + 一般的な build / implement / add 系
    for (needle, _, mode) in OPERATION_KEYWORDS {
        if keyword_hit(lower, needle, *mode) {
            return true;
        }
    }
    const EXTRA_VERBS: &[&str] = &[
        "build ",
        "implement ",
        "add ",
        "introduce ",
        "extend ",
        "support ",
        "expose ",
        "make ",
        "fix ",
    ];
    for v in EXTRA_VERBS {
        if lower.contains(v) {
            return true;
        }
    }
    false
}

/// `LABEL_MAX_LEN` byte で UTF-8 char boundary を維持しつつ切り詰める。
fn truncate_to_label(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.len() <= LABEL_MAX_LEN {
        return trimmed.to_string();
    }
    let mut end = LABEL_MAX_LEN;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
}

/// `EXCERPT_MAX_LEN` byte で UTF-8 char boundary を維持しつつ切り詰める。
fn truncate_excerpt(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.len() <= EXCERPT_MAX_LEN {
        return trimmed.to_string();
    }
    let mut end = EXCERPT_MAX_LEN;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
}

fn operation_in_request(op: Operation, lower: &str) -> bool {
    if lower.contains("crud")
        && matches!(
            op,
            Operation::Create | Operation::Read | Operation::Update | Operation::Delete
        )
    {
        return true;
    }
    for (needle, candidate, mode) in OPERATION_KEYWORDS {
        if *candidate == op && keyword_hit(lower, needle, *mode) {
            return true;
        }
    }
    false
}

fn verification_in_request(kind: VerificationKind, lower: &str) -> bool {
    for (needle, candidate, mode) in VERIFICATION_KEYWORDS {
        if *candidate == kind && keyword_hit(lower, needle, *mode) {
            return true;
        }
    }
    false
}

fn interface_hint_in_request(hint: InterfaceHint, lower: &str) -> bool {
    for (needle, candidate, mode) in INTERFACE_HINT_KEYWORDS {
        if *candidate == hint && keyword_hit(lower, needle, *mode) {
            return true;
        }
    }
    false
}

fn filter_domain_terms(terms: &[String], scan: &str) -> Option<Vec<String>> {
    let mut kept: Vec<String> = Vec::new();
    for term in terms {
        if term.len() > MAX_STR {
            continue;
        }
        if contains_redaction_sentinel(term) {
            continue;
        }
        if crate::session::feedback::mask_secrets(term) != *term {
            continue;
        }
        if !scan.contains(term.as_str()) {
            continue;
        }
        if kept.iter().any(|existing| existing == term) {
            continue;
        }
        kept.push(term.clone());
        if kept.len() >= MAX_ARRAY {
            break;
        }
    }
    if kept.is_empty() { None } else { Some(kept) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Group A: schema validation (5 tests)
    // -----------------------------------------------------------------

    fn empty_contract() -> RequiredBehaviorContract {
        RequiredBehaviorContract {
            operations: None,
            domain_terms: None,
            interface_hints: None,
            required_artifacts: None,
            verification: None,
            confidence: 0.0,
            // Issue #651: explicit default at every literal site so
            // adding a new boolean stays compile-time visible.
            test_execution_required: false,
            // Issue #665: same compile-time-visible policy (DR2-006).
            behavior_goal: None,
            required_capabilities: None,
            verification_expectations: None,
            non_goals: None,
        }
    }

    #[test]
    fn validate_rejects_confidence_above_one() {
        let mut c = empty_contract();
        c.confidence = 1.5;
        assert!(matches!(
            c.validate(),
            Err(SchemaError::ConfidenceInvalid { .. })
        ));
    }

    #[test]
    fn validate_rejects_nan_confidence() {
        let mut c = empty_contract();
        c.confidence = f32::NAN;
        // f32::NAN != f32::NAN, so use matches! + is_nan() check.
        match c.validate() {
            Err(SchemaError::ConfidenceInvalid { value }) => {
                assert!(value.is_nan(), "expected NaN, got {value}");
            }
            other => panic!("expected ConfidenceInvalid, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_too_many_operations() {
        let mut c = empty_contract();
        c.operations = Some(vec![
            Operation::Create,
            Operation::Read,
            Operation::Update,
            Operation::Delete,
            Operation::Run,
            Operation::Validate,
            Operation::Create,
            Operation::Read,
            Operation::Update,
        ]);
        assert!(matches!(
            c.validate(),
            Err(SchemaError::ArrayTooLong {
                field: "operations",
                ..
            })
        ));
    }

    #[test]
    fn validate_rejects_long_domain_term() {
        let mut c = empty_contract();
        c.domain_terms = Some(vec!["a".repeat(MAX_STR + 1)]);
        assert!(matches!(
            c.validate(),
            Err(SchemaError::DomainTermTooLong { .. })
        ));
    }

    #[test]
    fn validate_accepts_valid_minimal_schema() {
        let c = empty_contract();
        assert!(c.validate().is_ok());
        let mut c = empty_contract();
        c.confidence = 1.0;
        c.operations = Some(vec![Operation::Create]);
        c.domain_terms = Some(vec!["Task".to_string()]);
        assert!(c.validate().is_ok());
    }

    // -----------------------------------------------------------------
    // Group F (Issue #665): SSOT 定数 + BoundedLabelWithExcerpt + SchemaError 拡張
    // -----------------------------------------------------------------

    /// Issue #665 — Task 1.2: SSOT 定数の値 invariant をテスト。
    #[test]
    fn issue665_ssot_constants_have_expected_values() {
        assert_eq!(LABEL_MAX_LEN, MAX_STR);
        assert_eq!(EXCERPT_MAX_LEN, LABEL_MAX_LEN * 4);
        assert!((LOW_CONFIDENCE_THRESHOLD - 0.5_f32).abs() < f32::EPSILON);
        assert_eq!(MAX_BEHAVIOR_CONTRACT_PROJECTION_BYTES, 1200);
    }

    /// Issue #665 — Task 1.1: `BoundedLabelWithExcerpt` の literal 構築と
    /// `PartialEq` 比較が成立する。
    #[test]
    fn issue665_bounded_label_with_excerpt_literal_construction() {
        let a = BoundedLabelWithExcerpt {
            label: "create".to_string(),
            excerpt: Some("create a Task".to_string()),
        };
        let b = BoundedLabelWithExcerpt {
            label: "create".to_string(),
            excerpt: Some("create a Task".to_string()),
        };
        assert_eq!(a, b);
        let c = BoundedLabelWithExcerpt {
            label: "create".to_string(),
            excerpt: None,
        };
        assert_ne!(a, c);
    }

    /// Issue #665 — Task 1.3: `SchemaError::LabelTooLong` の `Display` 出力
    /// が新フィールド名と長さを含む。
    #[test]
    fn issue665_schema_error_label_too_long_display() {
        let err = SchemaError::LabelTooLong {
            field: "behavior_goal",
            len: 100,
        };
        let s = format!("{err}");
        assert!(s.contains("behavior_goal"), "got: {s}");
        assert!(s.contains("100"), "got: {s}");
        assert!(s.contains(&LABEL_MAX_LEN.to_string()), "got: {s}");
    }

    /// Issue #665 — Task 1.3: `SchemaError::ExcerptTooLong` の `Display`
    /// 出力が field 名 / 長さ / cap 値を含む。
    #[test]
    fn issue665_schema_error_excerpt_too_long_display() {
        let err = SchemaError::ExcerptTooLong {
            field: "non_goals",
            len: 500,
        };
        let s = format!("{err}");
        assert!(s.contains("non_goals"), "got: {s}");
        assert!(s.contains("500"), "got: {s}");
        assert!(s.contains(&EXCERPT_MAX_LEN.to_string()), "got: {s}");
    }

    /// Issue #665 — Task 2.2: `validate()` が新フィールド `behavior_goal` の
    /// label 超過を検出する。
    #[test]
    fn issue665_validate_rejects_oversize_behavior_goal_label() {
        let mut c = empty_contract();
        c.behavior_goal = Some(BoundedLabelWithExcerpt {
            label: "a".repeat(LABEL_MAX_LEN + 1),
            excerpt: None,
        });
        assert!(matches!(
            c.validate(),
            Err(SchemaError::LabelTooLong {
                field: "behavior_goal",
                ..
            })
        ));
    }

    /// Issue #665 — Task 2.2: `validate()` が `behavior_goal` の excerpt
    /// 超過を検出する。
    #[test]
    fn issue665_validate_rejects_oversize_behavior_goal_excerpt() {
        let mut c = empty_contract();
        c.behavior_goal = Some(BoundedLabelWithExcerpt {
            label: "build".to_string(),
            excerpt: Some("x".repeat(EXCERPT_MAX_LEN + 1)),
        });
        assert!(matches!(
            c.validate(),
            Err(SchemaError::ExcerptTooLong {
                field: "behavior_goal",
                ..
            })
        ));
    }

    /// Issue #665 — Task 2.2: `validate()` が `required_capabilities` の
    /// 配列長超過を検出する（既存 `MAX_ARRAY` ルールが新フィールドにも適用）。
    #[test]
    fn issue665_validate_rejects_too_many_required_capabilities() {
        let mut c = empty_contract();
        c.required_capabilities = Some(
            (0..(MAX_ARRAY + 1))
                .map(|i| BoundedLabelWithExcerpt {
                    label: format!("cap{i}"),
                    excerpt: None,
                })
                .collect(),
        );
        assert!(matches!(
            c.validate(),
            Err(SchemaError::ArrayTooLong {
                field: "required_capabilities",
                ..
            })
        ));
    }

    /// Issue #665 — Task 2.2: `validate()` が `verification_expectations` の
    /// 個別 label 超過を検出する（`field` SSOT 名前空間の検証）。
    #[test]
    fn issue665_validate_rejects_oversize_verification_expectation_label() {
        let mut c = empty_contract();
        c.verification_expectations = Some(vec![BoundedLabelWithExcerpt {
            label: "x".repeat(LABEL_MAX_LEN + 1),
            excerpt: None,
        }]);
        assert!(matches!(
            c.validate(),
            Err(SchemaError::LabelTooLong {
                field: "verification_expectations",
                ..
            })
        ));
    }

    /// Issue #665 — Task 2.2: `validate()` は valid な新フィールド構成を
    /// 受け入れる（境界値: label = `LABEL_MAX_LEN` ぴったり、excerpt =
    /// `EXCERPT_MAX_LEN` ぴったり）。
    #[test]
    fn issue665_validate_accepts_boundary_lengths_for_new_fields() {
        let mut c = empty_contract();
        c.behavior_goal = Some(BoundedLabelWithExcerpt {
            label: "a".repeat(LABEL_MAX_LEN),
            excerpt: Some("b".repeat(EXCERPT_MAX_LEN)),
        });
        c.required_capabilities = Some(vec![BoundedLabelWithExcerpt {
            label: "c".repeat(LABEL_MAX_LEN),
            excerpt: None,
        }]);
        c.verification_expectations = Some(vec![BoundedLabelWithExcerpt {
            label: "d".repeat(LABEL_MAX_LEN),
            excerpt: None,
        }]);
        c.non_goals = Some(vec![BoundedLabelWithExcerpt {
            label: "e".repeat(LABEL_MAX_LEN),
            excerpt: Some("f".repeat(EXCERPT_MAX_LEN)),
        }]);
        assert!(c.validate().is_ok(), "validate failed: {:?}", c.validate());
    }

    /// Issue #665 — Task 2.1 / 2.2: 既存 extract が新フィールド追加後も
    /// 破壊されない（既存 `operations` が抽出され、`validate()` が通る）。
    /// Phase 3 で 4 新フィールドも populated されるようになったが、本テストは
    /// **既存 extract 経路の不変** を主な確認対象とする。
    #[test]
    fn issue665_extract_keeps_existing_extraction_intact_after_new_fields() {
        let c = extract("Create a Task API");
        // 既存フィールドの抽出は不変であることを確認。
        assert!(
            c.operations.is_some(),
            "existing extract for `create` broken"
        );
        // validate() が新フィールド追加後も pass する。
        assert!(c.validate().is_ok());
    }

    // -----------------------------------------------------------------
    // Group G (Issue #665 Phase 3): extract_behavior_goal /
    // extract_non_goals / derive_required_capabilities /
    // derive_verification_expectations
    // -----------------------------------------------------------------

    /// Phase 3 / Task 3.1: imperative 文があれば behavior_goal として抽出。
    #[test]
    fn issue665_extract_behavior_goal_finds_imperative_sentence() {
        let c = extract("Build a Task API");
        let goal = c.behavior_goal.expect("behavior_goal should be Some");
        assert!(
            goal.label.to_ascii_lowercase().contains("build"),
            "label = {}",
            goal.label
        );
        assert!(goal.excerpt.is_some());
    }

    #[test]
    fn issue665_extract_behavior_goal_prefers_explicit_contract_sentence() {
        let c = extract(
            "TDD task. First create pytest tests in tests/test_password_strength.py. \
             Scoring contract: empty string is 0; add 1 point each for length at least 8, uppercase, lowercase, digit, symbol; cap at 5.",
        );
        let goal = c.behavior_goal.expect("behavior_goal should be Some");
        assert!(
            goal.excerpt
                .as_deref()
                .unwrap_or_default()
                .contains("Scoring contract"),
            "goal = {goal:?}"
        );
        assert!(
            goal.excerpt
                .as_deref()
                .unwrap_or_default()
                .contains("cap at 5"),
            "goal = {goal:?}"
        );
    }

    /// Phase 3 / Task 3.1: 否定文だけの request では behavior_goal は抽出しない。
    #[test]
    fn issue665_extract_behavior_goal_skips_negation_only_request() {
        // 否定文のみ。imperative verb は含まれていない。
        let c = extract("do not break existing tests");
        assert!(c.behavior_goal.is_none(), "got: {:?}", c.behavior_goal);
    }

    /// Phase 3 / Task 3.2: 否定文から non_goals を抽出。
    #[test]
    fn issue665_extract_non_goals_finds_english_negation() {
        let c = extract("Build a Task API. Do not break existing tests.");
        let non_goals = c.non_goals.expect("non_goals should be Some");
        assert!(!non_goals.is_empty());
        assert!(
            non_goals
                .iter()
                .any(|g| g.label.to_ascii_lowercase().contains("do not")
                    || g.label.to_ascii_lowercase().contains("break")),
            "got: {non_goals:?}"
        );
    }

    /// Phase 3 / Task 3.2: 日本語の否定パターンも non_goals に取れる。
    #[test]
    fn issue665_extract_non_goals_finds_japanese_negation() {
        let c = extract("Build a Task API。 既存テストは対象外。");
        let non_goals = c.non_goals.expect("non_goals should be Some");
        assert!(
            non_goals
                .iter()
                .any(|g| g.label.contains("対象外") || g.label.contains("テスト")),
            "got: {non_goals:?}"
        );
    }

    /// Phase 3 / Task 3.3 / S5-003: required_capabilities は operations と
    /// domain_terms から derive され、`excerpt` は `None`。
    #[test]
    fn issue665_required_capabilities_are_derived_with_no_excerpt() {
        let c = extract("Create a Task API");
        let caps = c
            .required_capabilities
            .expect("required_capabilities should be Some");
        assert!(!caps.is_empty());
        // すべての derived field の excerpt は None
        for cap in &caps {
            assert!(
                cap.excerpt.is_none(),
                "derived field must have excerpt: None (S5-003), got: {cap:?}"
            );
        }
        // operation `create` の label が含まれる
        assert!(caps.iter().any(|c| c.label == "create"), "got: {caps:?}");
    }

    /// Phase 3 / Task 3.3 / S5-003: verification_expectations も derived で
    /// excerpt: None。
    #[test]
    fn issue665_verification_expectations_are_derived_with_no_excerpt() {
        let c = extract("Build a Task API and run the tests");
        let ve = c
            .verification_expectations
            .expect("verification_expectations should be Some");
        assert!(!ve.is_empty());
        for v in &ve {
            assert!(
                v.excerpt.is_none(),
                "derived field must have excerpt: None (S5-003), got: {v:?}"
            );
        }
        // verification keywords `test` または `run` のいずれかの label が含まれる
        assert!(
            ve.iter().any(|v| v.label == "test" || v.label == "run"),
            "got: {ve:?}"
        );
    }

    /// Phase 3 / Task 3.4 / S5-002: filter_against_request は candidate の
    /// derived field を信頼せず、post-filter source fields から再生成する。
    #[test]
    fn issue665_filter_recomputes_derived_fields_from_post_filter_source() {
        // candidate が `required_capabilities=Some([fake_cap])` を持つが
        // `operations=None` という mismatched 状態。
        let candidate = RequiredBehaviorContract {
            operations: None,
            domain_terms: None,
            interface_hints: None,
            required_artifacts: None,
            verification: None,
            confidence: 1.0,
            test_execution_required: false,
            behavior_goal: None,
            required_capabilities: Some(vec![BoundedLabelWithExcerpt {
                label: "fake_cap_drifted".to_string(),
                excerpt: Some("attacker controlled".to_string()),
            }]),
            verification_expectations: Some(vec![BoundedLabelWithExcerpt {
                label: "fake_verification_drifted".to_string(),
                excerpt: Some("attacker controlled".to_string()),
            }]),
            non_goals: None,
        };
        // request 自体には operation も verification も無いので、再生成した
        // derived field は None に縮退するはず。
        let filtered = filter_against_request(&candidate, "explain something");
        assert!(
            filtered.required_capabilities.is_none()
                || filtered
                    .required_capabilities
                    .as_ref()
                    .is_none_or(|v| !v.iter().any(|e| e.label == "fake_cap_drifted")),
            "candidate-side fake_cap_drifted must NOT survive: got {:?}",
            filtered.required_capabilities
        );
        assert!(
            filtered.verification_expectations.is_none()
                || filtered
                    .verification_expectations
                    .as_ref()
                    .is_none_or(|v| !v.iter().any(|e| e.label == "fake_verification_drifted")),
            "candidate-side fake_verification_drifted must NOT survive: got {:?}",
            filtered.verification_expectations
        );
    }

    /// Phase 3 / Task 3.1: secret-like content (REDACTION_SENTINELS) を
    /// 含む sentence は behavior_goal に採用されない。
    #[test]
    fn issue665_extract_behavior_goal_drops_redaction_sentinel() {
        // ***（マスク後の sentinel）が含まれる sentence は drop。
        let c = extract("Build *** something secret");
        // 後続の sentence が無いので behavior_goal は None になる。
        assert!(c.behavior_goal.is_none(), "got: {:?}", c.behavior_goal);
    }

    /// Phase 3 / Task 3.2: non_goals は MAX_ARRAY 件で cap される。
    #[test]
    fn issue665_extract_non_goals_caps_at_max_array() {
        // 否定文を 10 個含む request を作る。
        let mut request = String::new();
        for i in 0..10 {
            request.push_str(&format!("do not break feature_{i}. "));
        }
        let c = extract(&request);
        let non_goals = c.non_goals.expect("non_goals should be Some");
        assert!(non_goals.len() <= MAX_ARRAY);
    }

    // -----------------------------------------------------------------
    // Group H (Issue #665 Phase 4): BehaviorContractProjection +
    // project_behavior_contract accessor
    // -----------------------------------------------------------------

    /// Phase 4 / Task 4.1: `BehaviorContractProjection` の literal 構築と
    /// `PartialEq` 比較が成立する。`Eq` は派生されない。
    #[test]
    fn issue665_behavior_contract_projection_literal_and_partial_eq() {
        let a = BehaviorContractProjection {
            confidence: 0.8,
            fields_used: vec!["behavior_goal"],
            behavior_goal: None,
            required_capabilities: vec![],
            verification_expectations: vec![],
            non_goals: vec![],
        };
        let b = a.clone();
        assert_eq!(a, b);
    }

    /// Phase 4 / Task 4.2: high confidence + fields_used 非空 で
    /// projection は Some を返す。
    #[test]
    fn issue665_project_behavior_contract_returns_some_for_high_confidence_request() {
        use super::super::task_contract::TaskContract;
        let tc = TaskContract::from_request("Create a Task API and run the tests");
        let proj = project_behavior_contract(&tc);
        assert!(proj.is_some(), "expected Some, got: {proj:?}");
        let proj = proj.unwrap();
        assert!(proj.confidence >= LOW_CONFIDENCE_THRESHOLD);
        assert!(!proj.fields_used.is_empty());
    }

    #[test]
    fn broad_domain_terms_do_not_create_repair_authority() {
        use super::super::task_contract::TaskContract;
        let tc = TaskContract::from_request(
            "文字列スラッグ生成用のRustライブラリを開発してください。README.mdとcargo testで動くテストも実装してください。",
        );

        assert!(
            project_behavior_contract(&tc).is_some(),
            "domain/runtime labels may still be useful diagnostic context"
        );
        assert!(
            !behavior_contract_has_repair_authority(&tc),
            "domain terms alone must not authorize exact assertion repair"
        );
    }

    #[test]
    fn crud_operations_create_repair_authority() {
        use super::super::task_contract::TaskContract;
        let tc = TaskContract::from_request(
            "FastAPIでCRUD APIを作成してREADMEとテストも追加してください",
        );

        assert!(
            behavior_contract_has_repair_authority(&tc),
            "closed CRUD operation labels are strong enough repair authority"
        );
    }

    /// Phase 4 / Task 4.2 / Task 4.3: confidence < threshold (0.5) で None。
    #[test]
    fn issue665_project_behavior_contract_returns_none_for_low_confidence() {
        use super::super::task_contract::TaskContract;
        // 日本語のみ short request: 既存 extractor は keyword hit が 0 で
        // confidence = 0.0 になる。
        let tc = TaskContract::from_request("こんにちは");
        let proj = project_behavior_contract(&tc);
        assert!(
            proj.is_none(),
            "expected None for low-confidence request, got: {proj:?}"
        );
    }

    /// Phase 4 / Task 4.3: threshold 跨ぎの境界値テスト (0.499 → None,
    /// 0.5 → projection 試行 (fields_used 次第))。
    #[test]
    fn issue665_project_behavior_contract_threshold_boundary() {
        use super::super::task_contract::TaskContract;
        let mut tc = TaskContract::from_request("Create a Task API");
        // 強制的に閾値直下に
        tc.required_behavior.confidence = LOW_CONFIDENCE_THRESHOLD - 0.01;
        let proj = project_behavior_contract(&tc);
        assert!(proj.is_none(), "below threshold should be None");
        // 閾値ぴったり：threshold 以上は OK
        tc.required_behavior.confidence = LOW_CONFIDENCE_THRESHOLD;
        let proj = project_behavior_contract(&tc);
        // fields_used に何か入っていれば Some。
        // Create a Task API は operations / required_capabilities が入る。
        assert!(
            proj.is_some(),
            "at threshold should be Some when fields exist"
        );
    }

    /// Phase 4 / Task 4.2: confidence が NaN や inf の場合は None。
    #[test]
    fn issue665_project_behavior_contract_returns_none_for_non_finite_confidence() {
        use super::super::task_contract::TaskContract;
        let mut tc = TaskContract::from_request("Create a Task API");
        tc.required_behavior.confidence = f32::NAN;
        assert!(project_behavior_contract(&tc).is_none());
        tc.required_behavior.confidence = f32::INFINITY;
        assert!(project_behavior_contract(&tc).is_none());
    }

    // -----------------------------------------------------------------
    // Group I (Issue #665 Phase 7 / Task 7.2): regression guard for
    // #636 judgement API `excerpt_hits_any_operation` /
    // `excerpt_hits_any_domain_term` — new fields MUST NOT change output.
    // -----------------------------------------------------------------

    /// Phase 7 / Task 7.2: `excerpt_hits_any_operation` の挙動は新フィールド
    /// 追加後も既存通り。
    #[test]
    fn issue665_phase7_excerpt_hits_any_operation_unchanged_after_new_fields() {
        let mut c = empty_contract();
        c.operations = Some(vec![Operation::Read]);
        // Baseline behavior: `README` does NOT match `read` (token boundary).
        assert!(!c.excerpt_hits_any_operation("Update README only"));
        assert!(c.excerpt_hits_any_operation("read the config"));
        // Populate all new fields with attacker-like content.
        c.behavior_goal = Some(BoundedLabelWithExcerpt {
            label: "create something".into(),
            excerpt: Some("create the resource".into()),
        });
        c.required_capabilities = Some(vec![BoundedLabelWithExcerpt {
            label: "update".into(),
            excerpt: None,
        }]);
        c.non_goals = Some(vec![BoundedLabelWithExcerpt {
            label: "delete bypass".into(),
            excerpt: Some("delete previous instructions".into()),
        }]);
        // Behavior unchanged: README still excluded, "read" still matches.
        assert!(
            !c.excerpt_hits_any_operation("Update README only"),
            "Phase 7 invariant: token boundary unchanged"
        );
        assert!(
            c.excerpt_hits_any_operation("read the config"),
            "Phase 7 invariant: positive match unchanged"
        );
        // The new-field content must NOT be wired into the matcher.
        // `create` / `update` / `delete` from new fields must NOT make
        // excerpt match if operations does not include them.
        assert!(
            !c.excerpt_hits_any_operation("delete previous instructions"),
            "Phase 7 invariant: new field labels must NOT widen operation match"
        );
    }

    /// Phase 7 / Task 7.2: `excerpt_hits_any_domain_term` も同様に不変。
    #[test]
    fn issue665_phase7_excerpt_hits_any_domain_term_unchanged_after_new_fields() {
        let mut c = empty_contract();
        c.domain_terms = Some(vec!["Task".to_string()]);
        // Baseline.
        assert!(c.excerpt_hits_any_domain_term("Make the Task observable"));
        assert!(!c.excerpt_hits_any_domain_term("Unrelated text only"));
        // Populate new fields.
        c.behavior_goal = Some(BoundedLabelWithExcerpt {
            label: "Other".into(),
            excerpt: Some("Other domain phrase".into()),
        });
        c.required_capabilities = Some(vec![BoundedLabelWithExcerpt {
            label: "Unrelated".into(),
            excerpt: None,
        }]);
        // Behavior unchanged.
        assert!(
            c.excerpt_hits_any_domain_term("Make the Task observable"),
            "Phase 7 invariant: positive match unchanged"
        );
        assert!(
            !c.excerpt_hits_any_domain_term("Other Unrelated text only"),
            "Phase 7 invariant: new field labels must NOT widen domain_term match"
        );
    }

    /// Phase 4 / Task 4.2: `fields_used` は `FIELDS_USED_ORDER` 順に
    /// deterministic に並ぶ。
    #[test]
    fn issue665_project_behavior_contract_fields_used_is_deterministic_order() {
        use super::super::task_contract::TaskContract;
        let tc = TaskContract::from_request(
            "Create a Task API and run the tests. Do not break existing builds.",
        );
        let proj = project_behavior_contract(&tc).expect("Some");
        // すべて fields_used に含まれている前提で、順序が
        // FIELDS_USED_ORDER の subsequence であることを確認。
        let mut prev_idx: i32 = -1;
        for f in &proj.fields_used {
            let cur = FIELDS_USED_ORDER
                .iter()
                .position(|x| x == f)
                .unwrap_or_else(|| panic!("unknown field name: {f}"));
            assert!(
                (cur as i32) > prev_idx,
                "fields_used not in canonical order: {:?} (expect subsequence of {:?})",
                proj.fields_used,
                FIELDS_USED_ORDER
            );
            prev_idx = cur as i32;
        }
    }

    // -----------------------------------------------------------------
    // Group B: extract + filter (8 tests)
    // -----------------------------------------------------------------

    #[test]
    fn extract_finds_quoted_domain_term() {
        let c = extract("Build a `Task` API with create/read");
        let terms = c.domain_terms.unwrap_or_default();
        assert!(terms.contains(&"Task".to_string()), "got: {terms:?}");
    }

    #[test]
    fn extract_does_not_treat_readme_as_read_operation() {
        let c = extract("Add a README file");
        let ops = c.operations.unwrap_or_default();
        assert!(!ops.contains(&Operation::Read), "got ops: {ops:?}");
    }

    #[test]
    fn extract_returns_none_for_free_form_domain_term() {
        let c = extract("please make me happy");
        assert!(c.domain_terms.is_none(), "got: {:?}", c.domain_terms);
    }

    #[test]
    fn extract_handles_long_request_within_scan_budget() {
        // Place an identifier-shaped token beyond the scan budget; it
        // must NOT make it into domain_terms because we cap the scan.
        let padding = "a".repeat(MAX_REQUEST_SCAN_BYTES + 128);
        let request = format!("{padding} `PastCap`");
        let c = extract(&request);
        let terms = c.domain_terms.unwrap_or_default();
        assert!(
            !terms.contains(&"PastCap".to_string()),
            "expected PastCap to be beyond scan budget, got: {terms:?}"
        );
    }

    #[test]
    fn extract_does_not_keep_raw_secret_like_token() {
        let c = extract("Build `Task` with API_KEY=sk-proj-aaaaaaaaaaaaaaaaaaaaaaaa");
        let terms = c.domain_terms.unwrap_or_default();
        let joined = terms.join("\n");
        assert!(!joined.contains("sk-proj-"), "got: {terms:?}");
        assert!(!joined.contains("API_KEY=***"), "got: {terms:?}");
        assert!(!joined.contains("***"), "got: {terms:?}");
    }

    #[test]
    fn filter_against_request_drops_unbacked_term() {
        let candidate = RequiredBehaviorContract {
            operations: Some(vec![Operation::Create, Operation::Delete]),
            domain_terms: Some(vec!["Foo".to_string(), "Bar".to_string()]),
            interface_hints: None,
            required_artifacts: None,
            verification: None,
            confidence: 1.0,
            test_execution_required: false,
            // Issue #665: explicit None per DR2-006 (literal site policy).
            behavior_goal: None,
            required_capabilities: None,
            verification_expectations: None,
            non_goals: None,
        };
        // Request mentions create + Foo only; Delete / Bar are unbacked.
        let filtered = filter_against_request(&candidate, "create a Foo");
        assert_eq!(filtered.operations.unwrap(), vec![Operation::Create]);
        assert_eq!(filtered.domain_terms.unwrap(), vec!["Foo".to_string()]);
    }

    #[test]
    fn filter_against_request_truncates_oversized_candidate() {
        let oversized = "x".repeat(MAX_STR + 1);
        let candidate = RequiredBehaviorContract {
            operations: None,
            domain_terms: Some(vec![oversized.clone()]),
            interface_hints: None,
            required_artifacts: None,
            verification: None,
            confidence: 1.0,
            test_execution_required: false,
            // Issue #665: explicit None per DR2-006 (literal site policy).
            behavior_goal: None,
            required_capabilities: None,
            verification_expectations: None,
            non_goals: None,
        };
        let request = format!("use {oversized}");
        let filtered = filter_against_request(&candidate, &request);
        assert!(
            filtered.domain_terms.is_none(),
            "oversized term must be dropped, got: {:?}",
            filtered.domain_terms
        );
    }

    #[test]
    fn filter_against_request_redacts_secret_in_candidate() {
        let token = "sk-proj-aaaaaaaaaaaaaaaaaaaaaaaa".to_string();
        let candidate = RequiredBehaviorContract {
            operations: None,
            domain_terms: Some(vec![token.clone()]),
            interface_hints: None,
            required_artifacts: None,
            verification: None,
            confidence: 1.0,
            test_execution_required: false,
            // Issue #665: explicit None per DR2-006 (literal site policy).
            behavior_goal: None,
            required_capabilities: None,
            verification_expectations: None,
            non_goals: None,
        };
        let filtered = filter_against_request(&candidate, &format!("use token {token}"));
        assert!(
            filtered.domain_terms.is_none(),
            "secret-like candidate must be dropped, got: {:?}",
            filtered.domain_terms
        );
    }

    // -----------------------------------------------------------------
    // Group C: integration + signature lock (2 tests)
    // -----------------------------------------------------------------

    #[test]
    fn unknown_required_artifacts_does_not_clear_existing_gates() {
        // Free-form code work request: existing gate should require
        // Implementation. Behavior schema is allowed to leave its own
        // required_artifacts as None / partial, but the TaskContract
        // gate must still be populated by the existing deterministic path.
        let contract =
            super::super::task_contract::TaskContract::from_request("implement feature X");
        assert!(
            contract
                .required_artifacts
                .contains(&super::super::task_contract::ArtifactRole::Implementation),
            "got: {:?}",
            contract.required_artifacts
        );
    }

    #[test]
    fn extract_signature_is_pure_function() {
        // Compile-time signature lock (DR1-007). These must NOT compile
        // if extract / filter_against_request ever start taking Agent /
        // SessionSnapshot or otherwise lose pure-function purity.
        let _: fn(&str) -> RequiredBehaviorContract = extract;
        let _: fn(&RequiredBehaviorContract, &str) -> RequiredBehaviorContract =
            filter_against_request;
    }

    // -----------------------------------------------------------------
    // Group D: Codex regression coverage (CB-001 .. CB-005)
    // -----------------------------------------------------------------

    /// CB-001 (High, potential DoS): the previous `collect_delimited_terms`
    /// rebuilt a `char_indices()` iterator on the suffix after each match
    /// but ignored the returned indices, so subsequent matches could either
    /// loop forever or visit the same delimiter twice. Multiple backtick /
    /// quote spans in a single request must be extracted in linear order
    /// without hanging and must terminate.
    #[test]
    fn extract_handles_multiple_delimited_domain_terms() {
        let c = extract("Build `Foo` and `Bar` then ship `Baz`");
        let terms = c.domain_terms.unwrap_or_default();
        assert!(terms.contains(&"Foo".to_string()), "got: {terms:?}");
        assert!(terms.contains(&"Bar".to_string()), "got: {terms:?}");
        assert!(terms.contains(&"Baz".to_string()), "got: {terms:?}");
    }

    /// CB-001 follow-up: mixed delimiters (backtick, single-quote,
    /// double-quote) must each yield their own terms when more than one
    /// span of each kind exists. This is the worst case for the previous
    /// iterator-restart bug because two `collect_delimited_terms` calls in
    /// a row both hit multiple-span input.
    #[test]
    fn extract_handles_mixed_multiple_delimiters() {
        let c = extract("`Alpha` 'Beta' \"Gamma\" `Delta` 'Epsilon' \"Zeta\"");
        let terms = c.domain_terms.unwrap_or_default();
        for expected in ["Alpha", "Beta", "Gamma", "Delta", "Epsilon", "Zeta"] {
            assert!(
                terms.contains(&expected.to_string()),
                "missing {expected}, got: {terms:?}"
            );
        }
    }

    /// CB-002 (High, secret leakage): a backtick-quoted `Authorization`
    /// header value short enough to fit MAX_STR was kept verbatim in
    /// `domain_terms`. Stacking `mask_header_family` on top of
    /// `mask_secrets` must redact the credential tail so no raw token
    /// reaches the schema.
    #[test]
    fn extract_drops_authorization_header_in_quoted_term() {
        let c = extract("Use `Authorization: Bearer abc123def456` in the API client");
        let terms = c.domain_terms.unwrap_or_default();
        let joined = terms.join("\n");
        assert!(
            !joined.contains("Bearer abc123def456"),
            "raw Authorization credential leaked: {terms:?}"
        );
        assert!(
            !joined.contains("abc123def456"),
            "raw credential tail leaked: {terms:?}"
        );
    }

    /// CB-002 follow-up: same coverage for Cookie header credentials.
    #[test]
    fn extract_drops_cookie_header_in_quoted_term() {
        let c = extract("Send `Cookie: session=abcdef123456` along with each request");
        let terms = c.domain_terms.unwrap_or_default();
        let joined = terms.join("\n");
        assert!(
            !joined.contains("session=abcdef123456"),
            "raw Cookie credential leaked: {terms:?}"
        );
    }

    /// CB-003 (Medium, DoS): the previous `extract_required_artifacts`
    /// passed the raw `request` straight to `request_asks_for_*` gates,
    /// so a 64KiB+ request with a trigger token past the scan cap still
    /// caused artifact detection. The bounded scan view must gate the
    /// substring search.
    #[test]
    fn extract_required_artifacts_respects_scan_budget() {
        // Place a Japanese "作成" trigger past the scan cap. Without the
        // bounded view, the gate fires; with it, it does not.
        let padding = "a".repeat(MAX_REQUEST_SCAN_BYTES + 64);
        let request = format!("{padding} 作成");
        let c = extract(&request);
        let arts = c.required_artifacts.unwrap_or_default();
        assert!(
            !arts.contains(&ArtifactKind::Implementation),
            "trigger past scan cap should not produce Implementation, got: {arts:?}"
        );
    }

    /// CB-004 (Medium): TaskContract treats Setup as **required** only
    /// when intent is `Install` (Setup-only request). When the request
    /// mixes Build + Setup (e.g. "FastAPI を作成して dependencies を追加")
    /// Setup is `optional` in TaskContract, so the behavior schema must
    /// not promote it to `required_artifacts` either.
    #[test]
    fn extract_setup_required_only_for_install_intent() {
        // Mixed Build + Setup: Setup must NOT be in required_artifacts.
        let c = extract(
            "FastAPI のサーバを作成して dependencies を requirements.txt に追加してください",
        );
        let arts = c.required_artifacts.unwrap_or_default();
        assert!(
            !arts.contains(&ArtifactKind::Setup),
            "Setup must be optional when intent is Build, got: {arts:?}"
        );
        assert!(
            arts.contains(&ArtifactKind::Implementation),
            "Implementation must still be required, got: {arts:?}"
        );

        // Pure Install intent: Setup IS required.
        let c2 = extract("Install the dependencies from requirements.txt");
        let arts2 = c2.required_artifacts.unwrap_or_default();
        assert!(
            arts2.contains(&ArtifactKind::Setup),
            "Setup must be required for pure Install intent, got: {arts2:?}"
        );
    }

    /// CB-004 round-2: the `!asks_for_impl` shortcut that the schema
    /// previously used to gate Setup diverged from
    /// `TaskContract::from_request` for requests like
    /// `"add dependencies to package.json"`:
    ///
    /// - support-aware `asks_for_impl` is `false` because the support
    ///   branch needs a `production_action` keyword and `add` is only
    ///   an `edit_action`,
    /// - but support-blind `request_asks_for_code_work` is `true` (the
    ///   `edit_action` branch picks up `add`), so `TaskContract` infers
    ///   intent = `Build`/`Modify` and keeps Setup `optional`.
    ///
    /// The fix routes the schema through the canonical
    /// `request_asks_for_code_work` so both paths agree: Setup stays
    /// off the required list here.
    #[test]
    fn extract_setup_not_required_for_modify_setup_request() {
        let c = extract("add dependencies to package.json");
        let arts = c.required_artifacts.unwrap_or_default();
        assert!(
            !arts.contains(&ArtifactKind::Setup),
            "Setup must be optional when the request has a code-work signal, got: {arts:?}"
        );
        // Cross-check against the canonical TaskContract path so the
        // two sources of truth cannot drift again.
        let contract = super::super::task_contract::TaskContract::from_request(
            "add dependencies to package.json",
        );
        assert!(
            !contract
                .required_artifacts
                .contains(&super::super::task_contract::ArtifactRole::Setup),
            "TaskContract must agree that Setup is optional here, got: {:?}",
            contract.required_artifacts
        );
    }

    /// CB-004 round-2: a pure Install request (Setup keyword, no code-work
    /// signal) must still promote Setup to `required_artifacts`. This is
    /// the positive counterpart to
    /// `extract_setup_not_required_for_modify_setup_request` and locks in
    /// the canonical `Install` rule
    /// `asks_for_setup && !request_asks_for_code_work`.
    #[test]
    fn extract_setup_required_for_install_request() {
        let c = extract("Install dependencies from requirements.txt");
        let arts = c.required_artifacts.unwrap_or_default();
        assert!(
            arts.contains(&ArtifactKind::Setup),
            "Setup must be required for pure Install intent, got: {arts:?}"
        );
        // Cross-check against the canonical TaskContract path.
        let contract = super::super::task_contract::TaskContract::from_request(
            "Install dependencies from requirements.txt",
        );
        assert!(
            contract
                .required_artifacts
                .contains(&super::super::task_contract::ArtifactRole::Setup),
            "TaskContract must agree that Setup is required here, got: {:?}",
            contract.required_artifacts
        );
    }

    /// CB-004 round-2: a pure Build request (e.g. "implement feature X")
    /// must NOT carry Setup in `required_artifacts` regardless of
    /// whether any Setup keyword appears — the schema and TaskContract
    /// agree that Setup is irrelevant when the request is code-work
    /// only. This pins the third corner of the Build / Modify / Install
    /// matrix together with the two tests above.
    #[test]
    fn extract_setup_absent_for_pure_build_request() {
        let c = extract("implement feature X");
        let arts = c.required_artifacts.unwrap_or_default();
        assert!(
            !arts.contains(&ArtifactKind::Setup),
            "Setup must not appear for a pure Build request, got: {arts:?}"
        );
        assert!(
            arts.contains(&ArtifactKind::Implementation),
            "Implementation must be required for a pure Build request, got: {arts:?}"
        );
    }

    /// CB-005 (Low): `filter_against_request` must not blindly trust
    /// `candidate.required_artifacts`. When the request does not ask for
    /// a Test artifact, a Test entry from the candidate must be dropped.
    #[test]
    fn filter_against_request_drops_unbacked_required_artifacts() {
        let candidate = RequiredBehaviorContract {
            operations: None,
            domain_terms: None,
            interface_hints: None,
            required_artifacts: Some(vec![ArtifactKind::Test, ArtifactKind::Implementation]),
            verification: None,
            confidence: 1.0,
            test_execution_required: false,
            // Issue #665: explicit None per DR2-006 (literal site policy).
            behavior_goal: None,
            required_capabilities: None,
            verification_expectations: None,
            non_goals: None,
        };
        // The request is an explain-only / read-only ask. It backs
        // neither Test nor Implementation, so both must be filtered out.
        let filtered = filter_against_request(&candidate, "explain Foo");
        let arts = filtered.required_artifacts.unwrap_or_default();
        assert!(
            !arts.contains(&ArtifactKind::Test),
            "unbacked Test must be dropped, got: {arts:?}"
        );
        assert!(
            !arts.contains(&ArtifactKind::Implementation),
            "unbacked Implementation must be dropped, got: {arts:?}"
        );
    }

    // -----------------------------------------------------------------
    // Group E: Issue #636 judgement APIs (5 tests)
    // -----------------------------------------------------------------

    fn contract_with_ops(ops: Vec<Operation>) -> RequiredBehaviorContract {
        let mut c = empty_contract();
        c.operations = Some(ops);
        c
    }

    fn contract_with_terms(terms: Vec<String>) -> RequiredBehaviorContract {
        let mut c = empty_contract();
        c.domain_terms = Some(terms);
        c
    }

    #[test]
    fn excerpt_hits_any_operation_matches_with_token_boundary_for_short_keyword() {
        let c = contract_with_ops(vec![Operation::Read]);
        // `README` should NOT count as a `read` operation.
        assert!(!c.excerpt_hits_any_operation("Update README only"));
        // Bare `read` token boundary matches.
        assert!(c.excerpt_hits_any_operation("we will Read the user record"));
        // `run` token boundary
        let c = contract_with_ops(vec![Operation::Run]);
        assert!(!c.excerpt_hits_any_operation("the running task"));
        assert!(c.excerpt_hits_any_operation("run the verifier"));
    }

    #[test]
    fn excerpt_hits_any_operation_matches_substring_for_long_keyword() {
        for op in [
            Operation::Create,
            Operation::Update,
            Operation::Delete,
            Operation::Validate,
        ] {
            let c = contract_with_ops(vec![op]);
            let excerpt = match op {
                Operation::Create => "We CREATE a new entity here",
                Operation::Update => "Update the existing record",
                Operation::Delete => "delete the row from storage",
                Operation::Validate => "Validate the payload structure",
                _ => unreachable!(),
            };
            assert!(
                c.excerpt_hits_any_operation(excerpt),
                "op {op:?} should match {excerpt:?}"
            );
        }
    }

    #[test]
    fn excerpt_hits_any_operation_returns_false_when_operations_is_none() {
        let c = empty_contract();
        assert!(!c.excerpt_hits_any_operation("create read update delete"));
        let mut c2 = empty_contract();
        c2.operations = Some(Vec::new());
        assert!(!c2.excerpt_hits_any_operation("create read update delete"));
    }

    #[test]
    fn excerpt_hits_any_domain_term_matches_case_insensitive() {
        let c = contract_with_terms(vec!["Task".to_string(), "api/v1".to_string()]);
        assert!(c.excerpt_hits_any_domain_term("def list_tasks():\n    return Task.all()"));
        assert!(c.excerpt_hits_any_domain_term("GET /api/v1/items"));
        assert!(!c.excerpt_hits_any_domain_term("nothing relevant here"));
    }

    #[test]
    fn excerpt_hits_any_domain_term_returns_false_when_domain_terms_is_none() {
        let c = empty_contract();
        assert!(!c.excerpt_hits_any_domain_term("Task api"));
        let mut c2 = empty_contract();
        c2.domain_terms = Some(Vec::new());
        assert!(!c2.excerpt_hits_any_domain_term("Task api"));
    }

    // -----------------------------------------------------------------
    // Group F (Issue #651): test_execution_required is SSOT-aligned
    // with `super::task_contract::request_asks_for_test_artifact`.
    // -----------------------------------------------------------------

    /// Helper: assert that `extract(request).test_execution_required`
    /// matches the SSOT predicate `request_asks_for_test_artifact`
    /// applied to the same bounded masked scan.
    fn assert_test_execution_required_matches_ssot(request: &str) {
        let c = extract(request);
        let scan = bounded_masked_request(request);
        let lower = scan.to_ascii_lowercase();
        let expected = super::super::task_contract::request_asks_for_test_artifact(&scan, &lower);
        assert_eq!(
            c.test_execution_required, expected,
            "request={request:?} test_execution_required diverged from SSOT predicate"
        );
        assert!(
            expected,
            "request={request:?} should request test execution (SSOT must be true)"
        );
    }

    #[test]
    fn extract_marks_test_execution_required_for_english_test_keyword() {
        assert_test_execution_required_matches_ssot("Please add a unit test for the API handler");
    }

    #[test]
    fn extract_marks_test_execution_required_for_pytest_keyword() {
        assert_test_execution_required_matches_ssot("write pytest cases for the new module");
    }

    #[test]
    fn extract_marks_test_execution_required_for_unittest_keyword() {
        assert_test_execution_required_matches_ssot("add unittest coverage for the parser");
    }

    #[test]
    fn extract_marks_test_execution_required_for_spec_keyword() {
        assert_test_execution_required_matches_ssot("write a spec describing the CRUD flow");
    }

    #[test]
    fn extract_marks_test_execution_required_for_japanese_test_keyword() {
        assert_test_execution_required_matches_ssot(
            "FastAPIでCRUDのAPIを開発してください。テストも実装してください。",
        );
    }

    // -----------------------------------------------------------------
    // Group G (Issue #652 / DR1-007): `requires_test_execution()` is a
    // direct read of `test_execution_required` after #651 landed the
    // explicit boolean.
    // -----------------------------------------------------------------

    #[test]
    fn requires_test_execution_returns_false_when_field_is_false() {
        let c = empty_contract();
        assert!(!c.requires_test_execution());
    }

    #[test]
    fn requires_test_execution_returns_true_when_field_is_true() {
        let mut c = empty_contract();
        c.test_execution_required = true;
        assert!(c.requires_test_execution());
    }

    #[test]
    fn requires_test_execution_is_independent_of_verification_kinds() {
        // After DR1-007 swap, `verification` is no longer read by the
        // helper — the explicit boolean is the SSOT.
        let mut c = empty_contract();
        c.verification = Some(vec![VerificationKind::Test]);
        c.test_execution_required = false;
        assert!(!c.requires_test_execution());

        c.verification = Some(vec![VerificationKind::Build, VerificationKind::Run]);
        c.test_execution_required = true;
        assert!(c.requires_test_execution());
    }

    // -----------------------------------------------------------------
    // Issue #664: Stage B substring helpers for SetupBootstrap signal.
    // -----------------------------------------------------------------

    fn projection_with_capabilities(labels: &[&str]) -> BehaviorContractProjection {
        let caps: Vec<BoundedLabelWithExcerpt> = labels
            .iter()
            .map(|s| BoundedLabelWithExcerpt {
                label: (*s).to_string(),
                excerpt: None,
            })
            .collect();
        BehaviorContractProjection {
            confidence: 0.8,
            fields_used: vec!["required_capabilities"],
            behavior_goal: None,
            required_capabilities: caps,
            verification_expectations: vec![],
            non_goals: vec![],
        }
    }

    fn projection_with_verification_expectations(labels: &[&str]) -> BehaviorContractProjection {
        let verifs: Vec<BoundedLabelWithExcerpt> = labels
            .iter()
            .map(|s| BoundedLabelWithExcerpt {
                label: (*s).to_string(),
                excerpt: None,
            })
            .collect();
        BehaviorContractProjection {
            confidence: 0.8,
            fields_used: vec!["verification_expectations"],
            behavior_goal: None,
            required_capabilities: vec![],
            verification_expectations: verifs,
            non_goals: vec![],
        }
    }

    #[test]
    fn behavior_projection_has_setup_label_detects_install_setup_dependency() {
        // Each needle must be detected from required_capabilities.
        for needle in [
            "install dependencies",
            "setup environment",
            "bootstrap project",
            "configure database",
            "manage dependency tree",
            "prepare environment",
        ] {
            let p = projection_with_capabilities(&[needle]);
            assert!(
                behavior_projection_has_setup_label(&p),
                "{needle:?} should match setup label"
            );
        }
    }

    #[test]
    fn behavior_projection_has_setup_label_is_false_for_no_match() {
        let p = projection_with_capabilities(&["draw chart", "render gui"]);
        assert!(!behavior_projection_has_setup_label(&p));
    }

    #[test]
    fn behavior_projection_has_verifier_capability_detects_test_verify() {
        let p = projection_with_capabilities(&["run test suite"]);
        assert!(behavior_projection_has_verifier_capability(&p));

        let p2 = projection_with_verification_expectations(&["verify outputs"]);
        assert!(behavior_projection_has_verifier_capability(&p2));
    }

    #[test]
    fn behavior_projection_has_verifier_capability_is_false_for_no_match() {
        let p = projection_with_capabilities(&["draw chart", "save file"]);
        assert!(!behavior_projection_has_verifier_capability(&p));
    }
}
