//! Issue #647 (Phase A.1): semantic repair planning — bounded failure report
//! schema and deterministic cluster-key generation.
//!
//! Visibility: every export here is `pub(super)` and **must not** be
//! re-exported from `src/agent/loop_run.rs` (CLAUDE.md DR3-001). The only
//! in-crate consumers will be `super::turn` (parse / dispatch boundary) and
//! `super::repair_job` (SemanticRepairPlan slot, arriving in Phase B).
//!
//! # Security: sanitize SSOT (DR1-001 / DR2-005)
//!
//! The two SSOT entry points (composed of the shared `mask_and_neutralize`
//! prefix in `repair_job.rs::mask_secrets_headers_and_neutralize`) are:
//!
//! - `sanitize_repair_job_text` — byte-bounded by `SNAPSHOT_FIELD_BYTE_CAP`,
//!   used by the snapshot store path.
//! - `sanitize_repair_job_text_with_char_cap(s, max_chars)` — char-bounded for
//!   parse-boundary fields with an explicit `max_chars`.
//!
//! Every `String` / `Vec<String>` field on `SemanticFailureReport` is run
//! through entry **(b)** at the parse boundary
//! (`parse_semantic_failure_report`) with `MAX_CLUSTER_TEXT_CHARS = 240`. The
//! lower-level `mask_secrets_headers_and_neutralize` is the **internal
//! prefix** of both entries and must **not** be called directly from a parse
//! boundary (avoid SSOT duplication).
//!
//! # Cluster key (DR4-001 / DR1-010)
//!
//! `FailureClusterKey` is a derived value, computed locally by
//! `build_failure_cluster_from_observation` over sanitized + shape-normalized
//! inputs. LLM-supplied `cluster_key` values are ignored (DR4-001) so that
//! diagnostic output cannot collide buckets, side-step `exhausted_attempts`
//! ledgers, or inject secret-derived fingerprints.
//!
//! `cluster_key` is module-private and asserts the sanitize precondition with
//! `debug_assert!` (caller contract enforced in dev/test, no-op in release).
//!
//! # Logging (DR4-004)
//!
//! Validation failures emit `tracing::warn!` payloads containing **only**
//! metadata (error code / field name / cap value). Raw observed / expected /
//! repair_hypothesis text never appears in log payloads.
//!
//! # Phase A.1 dead-code policy
//!
//! Phase A.1 lands the schema + parse + dispatch pieces ahead of their
//! consumers (Phase D wires `parse_semantic_failure_report` into the
//! diagnostic pipeline; Phase B stores `SemanticFailureReport` inside
//! `SemanticRepairPlan`). Until those Phases land, the
//! `pub(super)` surface is exercised only by in-module unit tests, so we
//! silence `dead_code` at the module root rather than dropping the API.

#![allow(dead_code)]

use super::VerifierDiagnosticFailureKind;
use super::repair_job::sanitize_repair_job_text_with_char_cap;
use super::task_contract::ArtifactRole;
use super::task_contract::RecoveryTargetHint;

/// Upper bound for `repair_hypothesis` after sanitize (chars, not bytes).
/// Mirrors the diagnostic summary cap used elsewhere in the agent loop.
pub(super) const MAX_REPAIR_HYPOTHESIS_CHARS: usize = 240;

/// Upper bound for the number of `affected_cases` retained per cluster.
pub(super) const MAX_AFFECTED_CASES: usize = 16;

/// Upper bound (chars) for shape-normalized cluster text fields
/// (`observed`, `expected`, `affected_cases[*]`, `ContractConflict.*`).
pub(super) const MAX_CLUSTER_TEXT_CHARS: usize = 240;

/// CB-017 A''': upper bound on the number of LLM-supplied raw target
/// candidates retained per cluster after parse. Anything beyond this is
/// truncated at the parse boundary so the rest of the pipeline operates on
/// a bounded list.
pub(super) const MAX_PROPOSED_TARGETS_PER_CLUSTER: usize = 8;

/// CB-017 A''': char-cap applied to each `RawClusterTargetCandidate.raw_path`
/// and `RawClusterTargetCandidate.reason` at the parse boundary via the
/// SSOT `sanitize_repair_job_text_with_char_cap` entry.
pub(super) const MAX_RAW_PATH_CHARS: usize = 240;

/// CB-017 A''': parse-stage advisory record for one LLM-proposed repair
/// target. Sanitized to bounded char caps but **not** Owned-validated — the
/// downstream `recovery_target_hint_for_diagnostic_path` SSOT decides
/// admission and is the source of truth for the resolved role.
///
/// `role_hint` is advisory only: it may be used as a tie-breaker in the
/// admission sort, or as debug/log metadata, but it never overrides the
/// path-classification role of an admitted hint (Invariant 2 in the design
/// policy).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RawClusterTargetCandidate {
    /// LLM-supplied `target_path` (sanitized, char-capped to
    /// `MAX_RAW_PATH_CHARS`; Owned admission is decided downstream).
    pub(super) raw_path: String,
    /// LLM-supplied `role`. Advisory only — admitted hint role is decided
    /// by `recovery_target_hint_for_existing_path` path classification.
    pub(super) role_hint: Option<ArtifactRole>,
    /// LLM-supplied raw diagnostic `reason` (sanitized + char-capped to
    /// `MAX_RAW_PATH_CHARS`). Must not be conflated with cluster id / role /
    /// authority strings.
    pub(super) reason: String,
}

/// Bounded, sanitized representation of a diagnostic LLM failure report.
///
/// All `String` / `Vec<String>` fields are sanitized at the parse boundary
/// so downstream consumers can read them without re-applying the SSOT
/// pipeline. `Eq` is intentionally dropped because `confidence: f32` is not
/// totally orderable (see `task_contract::TaskContract` for the same
/// trade-off).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SemanticFailureReport {
    pub(super) failure_kind: VerifierDiagnosticFailureKind,
    pub(super) failure_clusters: Vec<FailureCluster>,
    pub(super) contract_conflict: ContractConflict,
    pub(super) preferred_repair_role: ArtifactRole,
    /// Sanitized, char-capped to `MAX_REPAIR_HYPOTHESIS_CHARS`.
    pub(super) repair_hypothesis: String,
    /// Finite, in `[0.0, 1.0]` (validated at parse).
    pub(super) confidence: f32,
}

/// Shape-normalized, role-tagged grouping of failure observations that share
/// the same root cause. Cluster identity is encoded in `cluster_key`.
///
/// CB-017 A''' adds two `pub(super)` fields:
///   * `proposed_target_candidates` — parse-stage raw advisory candidates
///     (Owned-unvalidated). DR1-010: **not** included in the `cluster_key`
///     hash input.
///   * `admitted_cluster_targets` — Owned-validated `RecoveryTargetHint`s
///     produced by the turn.rs admission enrich step. Always empty at parse
///     time (`Vec::new()`); the turn.rs boundary fills this after
///     `recovery_target_hint_for_diagnostic_path` admission.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct FailureCluster {
    pub(super) cluster_key: FailureClusterKey,
    pub(super) observed: String,
    pub(super) expected: String,
    pub(super) affected_cases: Vec<String>,
    pub(super) involved_artifacts: Vec<ArtifactRole>,
    /// CB-017 A''': LLM-supplied raw target candidates (advisory).
    /// **Excluded from `cluster_key` hash input** (DR1-010).
    pub(super) proposed_target_candidates: Vec<RawClusterTargetCandidate>,
    /// CB-017 A''': admitted, Owned-validated targets filled in by the
    /// turn.rs enrich boundary. **Always empty at parse time**; downstream
    /// admission writes path-classification-derived roles here.
    pub(super) admitted_cluster_targets: Vec<RecoveryTargetHint>,
}

/// Deterministic 16-hex (sha256 short) identifier for a failure cluster.
///
/// Constructed **only** by `cluster_key` over sanitized inputs. The newtype
/// keeps the LLM-supplied raw string from ever escaping the parse boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct FailureClusterKey(String);

impl FailureClusterKey {
    /// Read-only access to the 16-hex representation.
    #[allow(dead_code)]
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Three-way contract conflict view: implementation vs. test vs. usage docs.
/// Each field is sanitized to `MAX_CLUSTER_TEXT_CHARS` at the parse boundary.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ContractConflict {
    pub(super) implementation: String,
    pub(super) test: String,
    pub(super) usage_docs: String,
}

/// Validation errors at the parse boundary.
///
/// Unit variants only — payloads (offending value, raw substrings) are
/// **never** attached so that `tracing::warn!` callsites cannot leak
/// secret-bearing text into logs (DR4-004). Eq derive is fine because no
/// `f32` is carried.
///
/// `ClusterKeyMalformed` is a defensive variant: it covers the case where a
/// locally generated key fails a debug invariant. It is **not** used to
/// reject LLM-supplied keys — those are discarded silently per DR4-001 and
/// regenerated by `build_failure_cluster_from_observation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SchemaError {
    ConfidenceInvalid,
    HypothesisTooLong,
    AffectedCasesTooMany,
    ClusterKeyMalformed,
}

impl SchemaError {
    /// Stable string code used as the only payload in `tracing::warn!`
    /// (DR4-004 — raw offending values must not be logged).
    pub(super) fn code(self) -> &'static str {
        match self {
            SchemaError::ConfidenceInvalid => "confidence_invalid",
            SchemaError::HypothesisTooLong => "hypothesis_too_long",
            SchemaError::AffectedCasesTooMany => "affected_cases_too_many",
            SchemaError::ClusterKeyMalformed => "cluster_key_malformed",
        }
    }
}

/// Dispatch target for a `SemanticFailureReport`.
///
/// `DependencyMissing` / `ConfigOrVerifierError` are routed to the existing
/// setup-repair / `MissingVerifierJob` pipeline; everything else continues
/// through semantic repair planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SemanticDispatchTarget {
    SemanticRepair,
    SetupRepair,
}

/// Public entry point that guarantees the
/// **sanitize → literal-to-shape normalize → cluster_key** invariant
/// (DR1-010 + design judgment #6 §4.2): every string is first run through
/// `sanitize_repair_job_text_with_char_cap` and then through
/// [`normalize_to_shape`] before the hash input is assembled, so
/// `cluster_key` only sees bounded, redacted, *shape-normalized* text.
///
/// The stored `observed` / `expected` on `FailureCluster` retain the
/// sanitized (but unnormalized) text so operators can read the diagnostic
/// in human form; only the hash-input variants are normalized.
///
/// `assertion_shape` is a caller-supplied label (e.g. the variant name of a
/// closed enum). The function does not interpret its contents — it only
/// participates in the cluster identity.
pub(super) fn build_failure_cluster_from_observation(
    raw_observed: &str,
    raw_expected: &str,
    raw_input_shape: &str,
    assertion_shape: &str,
    artifact_role_set: &[ArtifactRole],
    proposed_target_candidates: Vec<RawClusterTargetCandidate>,
) -> FailureCluster {
    let observed = sanitize_repair_job_text_with_char_cap(raw_observed, MAX_CLUSTER_TEXT_CHARS);
    let expected = sanitize_repair_job_text_with_char_cap(raw_expected, MAX_CLUSTER_TEXT_CHARS);
    let input_shape_signature =
        sanitize_repair_job_text_with_char_cap(raw_input_shape, MAX_CLUSTER_TEXT_CHARS);
    let assertion_shape_sig =
        sanitize_repair_job_text_with_char_cap(assertion_shape, MAX_CLUSTER_TEXT_CHARS);

    // CB-002: hash inputs are normalized to coarse shape so secret-like or
    // identifier-like literals never form a stable fingerprint and don't
    // fragment clusters that share the same shape.
    //
    // DR1-010 (CB-017 A'''): `proposed_target_candidates` are **not** fed
    // into `cluster_key` — cluster identity stays a pure function of the
    // shape-normalized observation, not LLM-supplied advisory paths.
    let observed_shape = normalize_to_shape(&observed);
    let expected_shape = normalize_to_shape(&expected);
    let input_shape_shape = normalize_to_shape(&input_shape_signature);
    let assertion_shape_shape = normalize_to_shape(&assertion_shape_sig);

    let mut involved_artifacts: Vec<ArtifactRole> = artifact_role_set.to_vec();
    involved_artifacts.sort();
    involved_artifacts.dedup();

    let key = cluster_key(
        &observed_shape,
        &expected_shape,
        &input_shape_shape,
        &assertion_shape_shape,
        &involved_artifacts,
    );

    FailureCluster {
        cluster_key: key,
        observed,
        expected,
        affected_cases: Vec::new(),
        involved_artifacts,
        // CB-017 A''': proposed candidates flow through as-is (advisory);
        // admitted targets are filled by the turn.rs enrich boundary.
        proposed_target_candidates,
        admitted_cluster_targets: Vec::new(),
    }
}

/// CB-002 / design §4.2: collapse literal values in `s` into coarse shape
/// tokens so that hash inputs of different but structurally similar
/// observations land in the same cluster (and so that secret-like values
/// never form a stable fingerprint).
///
/// Order of replacements (each is order-dependent — earlier rules win):
/// 1. UUID (8-4-4-4-12 hex) → `<uuid>`
/// 2. Quoted literals (`"..."` / `'...'`) → `<str>`
/// 3. Path segments (one or more `/segment/` runs) → `<path>`
/// 4. Long opaque tokens (16+ contiguous alnum/`_`/`-`) → `<token>`
/// 5. Decimal / hex / float number runs → `<num>`
/// 6. The remaining raw text is truncated to `MAX_RAW_AFTER_SHAPE`
///    characters so the hash input length is bounded.
///
/// Returned text is plain ASCII and contains no control characters (the
/// inputs already passed `mask_and_neutralize` upstream).
pub(super) fn normalize_to_shape(s: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;

    const MAX_RAW_AFTER_SHAPE: usize = 64;

    // Static one-time compiled regexes.
    static UUID_RE: OnceLock<Regex> = OnceLock::new();
    static QSTR_RE: OnceLock<Regex> = OnceLock::new();
    static PATH_RE: OnceLock<Regex> = OnceLock::new();
    static TOKEN_RE: OnceLock<Regex> = OnceLock::new();
    static NUM_RE: OnceLock<Regex> = OnceLock::new();

    let uuid = UUID_RE.get_or_init(|| {
        Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
            .expect("uuid regex compiles")
    });
    let qstr = QSTR_RE.get_or_init(|| {
        // Non-greedy quoted runs; allow escaped quotes.
        Regex::new(r#""(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'"#).expect("qstr regex compiles")
    });
    let path = PATH_RE.get_or_init(|| {
        // One or more `/<segment>` runs, where segment is non-whitespace
        // and contains no `/`. Anchor: leading `/`.
        Regex::new(r"(?:/[^/\s]+){1,}").expect("path regex compiles")
    });
    let token = TOKEN_RE.get_or_init(|| {
        // 16+ contiguous alnum/_/- characters → opaque token.
        Regex::new(r"[A-Za-z0-9_\-]{16,}").expect("token regex compiles")
    });
    let num = NUM_RE.get_or_init(|| {
        // Integer / hex / decimal float run. (Order matters: this is last
        // so numeric parts of larger tokens were already absorbed.)
        Regex::new(r"\b(?:0x[0-9A-Fa-f]+|\d+(?:\.\d+)?)\b").expect("num regex compiles")
    });

    let stage1 = uuid.replace_all(s, "<uuid>");
    let stage2 = qstr.replace_all(&stage1, "<str>");
    let stage3 = path.replace_all(&stage2, "<path>");
    let stage4 = token.replace_all(&stage3, "<token>");
    let stage5 = num.replace_all(&stage4, "<num>");

    // Truncate any remaining raw run to MAX_RAW_AFTER_SHAPE chars. We keep
    // a `<...>` suffix when truncating so distinct over-cap inputs don't
    // collide just because their first 64 chars matched.
    let truncated: String = stage5.chars().take(MAX_RAW_AFTER_SHAPE).collect();
    if stage5.chars().count() > MAX_RAW_AFTER_SHAPE {
        format!("{truncated}<...>")
    } else {
        truncated
    }
}

/// Module-private deterministic hash over sanitized + shape-normalized
/// components. Returns a 16-hex (sha256 short, 8 leading bytes) identifier.
///
/// The function is intentionally private — its single caller is
/// `build_failure_cluster_from_observation`, which is the only place where
/// the sanitize precondition is enforced. `debug_assert!` is used to flag
/// any future direct callers in dev/test builds.
fn cluster_key(
    observed_shape: &str,
    expected_shape: &str,
    input_shape_signature: &str,
    assertion_shape: &str,
    artifact_role_set: &[ArtifactRole],
) -> FailureClusterKey {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;

    debug_assert!(
        is_already_sanitized(observed_shape),
        "cluster_key precondition: observed must be sanitized"
    );
    debug_assert!(
        is_already_sanitized(expected_shape),
        "cluster_key precondition: expected must be sanitized"
    );
    debug_assert!(
        is_already_sanitized(input_shape_signature),
        "cluster_key precondition: input_shape must be sanitized"
    );
    debug_assert!(
        is_already_sanitized(assertion_shape),
        "cluster_key precondition: assertion_shape must be sanitized"
    );

    let mut roles_sorted: Vec<&str> = artifact_role_set.iter().map(|r| r.label()).collect();
    roles_sorted.sort();
    roles_sorted.dedup();

    let joined = format!(
        "{}|{}|{}|{}|{}",
        observed_shape,
        expected_shape,
        input_shape_signature,
        assertion_shape,
        roles_sorted.join(",")
    );
    let hash = Sha256::digest(joined.as_bytes());
    let mut hex = String::with_capacity(16);
    for byte in &hash[..8] {
        // 2 hex chars per byte → 16 chars total.
        let _ = write!(hex, "{:02x}", byte);
    }
    debug_assert_eq!(hex.len(), 16);
    FailureClusterKey(hex)
}

/// `debug_assert!` helper: returns `true` when the input has already passed
/// through the SSOT sanitize entry (no control chars, length ≤ cap).
///
/// This is a conservative invariant — it cannot prove that
/// `mask_secrets`/`mask_header_family` ran, but it catches the most common
/// programmer mistakes (raw control chars, oversized strings).
fn is_already_sanitized(s: &str) -> bool {
    if s.chars().count() > MAX_CLUSTER_TEXT_CHARS + 3 {
        // +3 accounts for the trailing "..." ellipsis appended by
        // `truncate_chars_with_ellipsis` when the input exceeded the cap.
        return false;
    }
    !s.chars().any(|c| (c as u32) < 0x20 || c == '\x7f')
}

/// Parse + validate a diagnostic LLM JSON payload into a bounded
/// `SemanticFailureReport`.
///
/// Returns `None` on any validation failure, after emitting a structured
/// `tracing::warn!` event carrying only the error code, field name, and cap
/// (DR4-004 — raw offending values are never logged).
///
/// LLM-supplied `failure_clusters[].cluster_key` values are ignored
/// (DR4-001): clusters are reconstructed from sanitized components via
/// `build_failure_cluster_from_observation`.
pub(super) fn parse_semantic_failure_report(
    json: &serde_json::Value,
) -> Option<SemanticFailureReport> {
    let obj = json.as_object()?;

    let failure_kind = parse_failure_kind(obj.get("failure_kind")?)?;

    // confidence: finite + within [0, 1].
    let confidence = obj
        .get("confidence")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)?;
    if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
        warn_schema_error(SchemaError::ConfidenceInvalid, "confidence", None);
        return None;
    }

    // CB-005 (Codex review) / S1-014: do NOT reject the entire report when
    // `repair_hypothesis` exceeds the cap. The SSOT sanitize entry
    // (`sanitize_repair_job_text_with_char_cap`) already truncates with an
    // ellipsis; rejecting here would throw away the whole diagnostic over a
    // single overly long free-form field. We still emit a structured warn
    // so operators can spot oversized hypotheses, but downstream consumers
    // receive the truncated text.
    let raw_hypothesis = obj
        .get("repair_hypothesis")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if raw_hypothesis.chars().count() > MAX_REPAIR_HYPOTHESIS_CHARS {
        warn_schema_error(
            SchemaError::HypothesisTooLong,
            "repair_hypothesis",
            Some(MAX_REPAIR_HYPOTHESIS_CHARS),
        );
    }
    let repair_hypothesis =
        sanitize_repair_job_text_with_char_cap(raw_hypothesis, MAX_REPAIR_HYPOTHESIS_CHARS);

    // preferred_repair_role.
    let preferred_repair_role = obj
        .get("preferred_repair_role")
        .and_then(|v| v.as_str())
        .and_then(parse_artifact_role)?;

    // contract_conflict.
    let cc = obj.get("contract_conflict").and_then(|v| v.as_object());
    let contract_conflict = ContractConflict {
        implementation: sanitize_repair_job_text_with_char_cap(
            cc.and_then(|o| o.get("implementation"))
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            MAX_CLUSTER_TEXT_CHARS,
        ),
        test: sanitize_repair_job_text_with_char_cap(
            cc.and_then(|o| o.get("test"))
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            MAX_CLUSTER_TEXT_CHARS,
        ),
        usage_docs: sanitize_repair_job_text_with_char_cap(
            cc.and_then(|o| o.get("usage_docs"))
                .and_then(|v| v.as_str())
                .unwrap_or(""),
            MAX_CLUSTER_TEXT_CHARS,
        ),
    };

    // failure_clusters: LLM-supplied cluster_key is ignored (DR4-001) and
    // every cluster is rebuilt from sanitized components.
    let mut failure_clusters: Vec<FailureCluster> = Vec::new();
    if let Some(arr) = obj.get("failure_clusters").and_then(|v| v.as_array()) {
        for cluster in arr {
            let cluster_obj = match cluster.as_object() {
                Some(o) => o,
                None => continue,
            };

            // affected_cases: bounded count, each sanitized.
            let mut affected_cases: Vec<String> = Vec::new();
            if let Some(cases) = cluster_obj.get("affected_cases").and_then(|v| v.as_array()) {
                if cases.len() > MAX_AFFECTED_CASES {
                    warn_schema_error(
                        SchemaError::AffectedCasesTooMany,
                        "affected_cases",
                        Some(MAX_AFFECTED_CASES),
                    );
                    return None;
                }
                for case in cases {
                    if let Some(s) = case.as_str() {
                        affected_cases.push(sanitize_repair_job_text_with_char_cap(
                            s,
                            MAX_CLUSTER_TEXT_CHARS,
                        ));
                    }
                }
            }

            let involved_artifacts: Vec<ArtifactRole> = cluster_obj
                .get("involved_artifacts")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().and_then(parse_artifact_role))
                        .collect()
                })
                .unwrap_or_default();

            let raw_observed = cluster_obj
                .get("observed")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let raw_expected = cluster_obj
                .get("expected")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let raw_input_shape = cluster_obj
                .get("input_shape")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let raw_assertion_shape = cluster_obj
                .get("assertion_shape")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // CB-017 A''': extract LLM-supplied `target_paths` advisory list.
            // Sanitized + char-capped + bounded-count at the parse boundary.
            // `admitted_cluster_targets` stays empty here — it is filled by
            // the turn.rs enrich step which is the SSOT for Owned admission.
            let proposed_target_candidates: Vec<RawClusterTargetCandidate> = cluster_obj
                .get("target_paths")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|entry| entry.as_object())
                        .take(MAX_PROPOSED_TARGETS_PER_CLUSTER)
                        .map(|entry| {
                            let raw_path = sanitize_repair_job_text_with_char_cap(
                                entry.get("path").and_then(|v| v.as_str()).unwrap_or(""),
                                MAX_RAW_PATH_CHARS,
                            );
                            let role_hint = entry
                                .get("role")
                                .and_then(|v| v.as_str())
                                .and_then(parse_artifact_role);
                            let reason = sanitize_repair_job_text_with_char_cap(
                                entry.get("reason").and_then(|v| v.as_str()).unwrap_or(""),
                                MAX_RAW_PATH_CHARS,
                            );
                            RawClusterTargetCandidate {
                                raw_path,
                                role_hint,
                                reason,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            let mut rebuilt = build_failure_cluster_from_observation(
                raw_observed,
                raw_expected,
                raw_input_shape,
                raw_assertion_shape,
                &involved_artifacts,
                proposed_target_candidates,
            );
            rebuilt.affected_cases = affected_cases;
            failure_clusters.push(rebuilt);
        }
    }

    Some(SemanticFailureReport {
        failure_kind,
        failure_clusters,
        contract_conflict,
        preferred_repair_role,
        repair_hypothesis,
        confidence,
    })
}

/// Route a parsed report to the correct repair pipeline.
///
/// `DependencyMissing` / `ConfigOrVerifierError` short-circuit to setup
/// repair (existing `MissingVerifierJob` / setup pipeline); every other
/// failure kind continues through semantic repair planning.
pub(super) fn dispatch_target(report: &SemanticFailureReport) -> SemanticDispatchTarget {
    match report.failure_kind {
        VerifierDiagnosticFailureKind::DependencyMissing
        | VerifierDiagnosticFailureKind::ConfigOrVerifierError => {
            SemanticDispatchTarget::SetupRepair
        }
        _ => SemanticDispatchTarget::SemanticRepair,
    }
}

/// Parse the closed set of `VerifierDiagnosticFailureKind` labels from the
/// existing SSOT (`loop_run.rs::VerifierDiagnosticFailureKind::as_str`).
fn parse_failure_kind(v: &serde_json::Value) -> Option<VerifierDiagnosticFailureKind> {
    Some(match v.as_str()? {
        "dependency_missing" => VerifierDiagnosticFailureKind::DependencyMissing,
        "local_import_contract_mismatch" => {
            VerifierDiagnosticFailureKind::LocalImportContractMismatch
        }
        "compile_or_syntax_error" => VerifierDiagnosticFailureKind::CompileOrSyntaxError,
        "assertion_mismatch" => VerifierDiagnosticFailureKind::AssertionMismatch,
        "runtime_error" => VerifierDiagnosticFailureKind::RuntimeError,
        "test_bug" => VerifierDiagnosticFailureKind::TestBug,
        "config_or_verifier_error" => VerifierDiagnosticFailureKind::ConfigOrVerifierError,
        "unknown" => VerifierDiagnosticFailureKind::Unknown,
        _ => return None,
    })
}

/// Parse the closed set of `ArtifactRole` labels from
/// `task_contract::ArtifactRole::label`.
fn parse_artifact_role(s: &str) -> Option<ArtifactRole> {
    Some(match s {
        "implementation" => ArtifactRole::Implementation,
        "test" => ArtifactRole::Test,
        "usage_docs" => ArtifactRole::UsageDocs,
        "setup" => ArtifactRole::Setup,
        _ => return None,
    })
}

/// Emit a `SchemaError` validation failure with **metadata only** — never the
/// offending raw value (DR4-004).
fn warn_schema_error(err: SchemaError, field: &'static str, cap: Option<usize>) {
    tracing::warn!(
        event = "agent.semantic_failure.schema_error",
        error_code = err.code(),
        field = field,
        cap = cap,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role_set(roles: &[ArtifactRole]) -> Vec<ArtifactRole> {
        roles.to_vec()
    }

    #[test]
    fn cluster_key_is_deterministic_for_same_inputs() {
        let a = build_failure_cluster_from_observation(
            "observed text",
            "expected text",
            "input shape",
            "AssertEq",
            &role_set(&[ArtifactRole::Test, ArtifactRole::Implementation]),
            Vec::new(),
        );
        let b = build_failure_cluster_from_observation(
            "observed text",
            "expected text",
            "input shape",
            "AssertEq",
            &role_set(&[ArtifactRole::Implementation, ArtifactRole::Test]),
            Vec::new(),
        );
        assert_eq!(a.cluster_key, b.cluster_key);
        assert_eq!(a.cluster_key.as_str().len(), 16);
    }

    #[test]
    fn cluster_key_changes_when_inputs_change() {
        let a = build_failure_cluster_from_observation(
            "observed",
            "expected",
            "shape",
            "AssertEq",
            &role_set(&[ArtifactRole::Test]),
            Vec::new(),
        );
        let b = build_failure_cluster_from_observation(
            "observed",
            "expected DIFFERENT",
            "shape",
            "AssertEq",
            &role_set(&[ArtifactRole::Test]),
            Vec::new(),
        );
        assert_ne!(a.cluster_key, b.cluster_key);
    }

    #[test]
    fn cluster_key_uses_sanitized_inputs() {
        // raw secret-like material should be masked before reaching cluster_key.
        let raw_with_secret =
            "Authorization: Bearer sk-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let cluster = build_failure_cluster_from_observation(
            raw_with_secret,
            "expected",
            "shape",
            "AssertEq",
            &role_set(&[ArtifactRole::Test]),
            Vec::new(),
        );
        // The sanitized observed text should not be byte-for-byte identical
        // to the raw secret-bearing input (mask_header_family / mask_secrets
        // applied at the boundary).
        assert_ne!(cluster.observed, raw_with_secret);
        // No control chars in the stored observed text.
        assert!(!cluster.observed.chars().any(|c| (c as u32) < 0x20));
    }

    #[test]
    fn cluster_key_role_order_does_not_change_identity() {
        // ArtifactRole input order must not affect the cluster identity.
        let key_a = build_failure_cluster_from_observation(
            "o",
            "e",
            "s",
            "AssertEq",
            &role_set(&[ArtifactRole::UsageDocs, ArtifactRole::Implementation]),
            Vec::new(),
        )
        .cluster_key;
        let key_b = build_failure_cluster_from_observation(
            "o",
            "e",
            "s",
            "AssertEq",
            &role_set(&[ArtifactRole::Implementation, ArtifactRole::UsageDocs]),
            Vec::new(),
        )
        .cluster_key;
        assert_eq!(key_a, key_b);
    }

    #[test]
    fn parse_rejects_nan_and_inf_confidence() {
        let nan = serde_json::json!({
            "failure_kind": "assertion_mismatch",
            "confidence": f64::NAN,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": "",
        });
        assert!(parse_semantic_failure_report(&nan).is_none());

        let inf = serde_json::json!({
            "failure_kind": "assertion_mismatch",
            "confidence": f64::INFINITY,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": "",
        });
        assert!(parse_semantic_failure_report(&inf).is_none());
    }

    #[test]
    fn parse_rejects_confidence_outside_unit_interval() {
        let low = serde_json::json!({
            "failure_kind": "assertion_mismatch",
            "confidence": -0.1,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": "",
        });
        assert!(parse_semantic_failure_report(&low).is_none());

        let high = serde_json::json!({
            "failure_kind": "assertion_mismatch",
            "confidence": 1.1,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": "",
        });
        assert!(parse_semantic_failure_report(&high).is_none());
    }

    #[test]
    fn parse_accepts_confidence_at_boundary() {
        for c in [0.0_f64, 1.0_f64] {
            let v = serde_json::json!({
                "failure_kind": "assertion_mismatch",
                "confidence": c,
                "preferred_repair_role": "implementation",
                "repair_hypothesis": "",
            });
            let parsed = parse_semantic_failure_report(&v).expect("boundary accepted");
            assert_eq!(parsed.confidence, c as f32);
        }
    }

    #[test]
    fn parse_truncates_hypothesis_longer_than_cap_cb005() {
        // CB-005 acceptance (S1-014): an over-cap repair_hypothesis must be
        // truncated via the SSOT sanitize entry, not cause the whole report
        // to be rejected. 300 chars → ≤ MAX_REPAIR_HYPOTHESIS_CHARS + ellipsis.
        let long = "a".repeat(300);
        let v = serde_json::json!({
            "failure_kind": "assertion_mismatch",
            "confidence": 0.5,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": long,
        });
        let parsed = parse_semantic_failure_report(&v)
            .expect("CB-005: long hypothesis must not reject the report");
        // Truncated length: cap + at most 3 trailing ellipsis chars
        // (truncate_chars_with_ellipsis convention).
        let n = parsed.repair_hypothesis.chars().count();
        assert!(
            n <= MAX_REPAIR_HYPOTHESIS_CHARS + 3,
            "truncated length {n} exceeded cap+ellipsis",
        );
        // And the truncated text must be a strict prefix of the raw "a"-run
        // (modulo the trailing ellipsis).
        assert!(parsed.repair_hypothesis.starts_with("aaa"));
    }

    #[test]
    fn parse_rejects_affected_cases_over_cap() {
        let cases: Vec<&str> = (0..(MAX_AFFECTED_CASES + 1)).map(|_| "case").collect();
        let v = serde_json::json!({
            "failure_kind": "assertion_mismatch",
            "confidence": 0.5,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": "h",
            "failure_clusters": [
                {
                    "observed": "o",
                    "expected": "e",
                    "input_shape": "s",
                    "assertion_shape": "AssertEq",
                    "involved_artifacts": ["test"],
                    "affected_cases": cases,
                }
            ],
        });
        assert!(parse_semantic_failure_report(&v).is_none());
    }

    #[test]
    fn parse_ignores_llm_supplied_cluster_key_and_regenerates() {
        let v = serde_json::json!({
            "failure_kind": "assertion_mismatch",
            "confidence": 0.5,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": "h",
            "failure_clusters": [
                {
                    "cluster_key": "deadbeefdeadbeef", // attacker-supplied
                    "observed": "o",
                    "expected": "e",
                    "input_shape": "s",
                    "assertion_shape": "AssertEq",
                    "involved_artifacts": ["test"],
                    "affected_cases": ["case1"],
                }
            ],
        });
        let parsed = parse_semantic_failure_report(&v).expect("parses");
        assert_eq!(parsed.failure_clusters.len(), 1);
        // The locally regenerated key must NOT equal the attacker payload.
        assert_ne!(
            parsed.failure_clusters[0].cluster_key.as_str(),
            "deadbeefdeadbeef"
        );
        // And it must match a freshly computed key from the same components.
        let regenerated = build_failure_cluster_from_observation(
            "o",
            "e",
            "s",
            "AssertEq",
            &[ArtifactRole::Test],
            Vec::new(),
        );
        assert_eq!(
            parsed.failure_clusters[0].cluster_key,
            regenerated.cluster_key
        );
    }

    #[test]
    fn parse_returns_none_for_unknown_failure_kind() {
        let v = serde_json::json!({
            "failure_kind": "totally_made_up",
            "confidence": 0.5,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": "h",
        });
        assert!(parse_semantic_failure_report(&v).is_none());
    }

    #[test]
    fn parse_returns_none_for_unknown_repair_role() {
        let v = serde_json::json!({
            "failure_kind": "assertion_mismatch",
            "confidence": 0.5,
            "preferred_repair_role": "controller",
            "repair_hypothesis": "h",
        });
        assert!(parse_semantic_failure_report(&v).is_none());
    }

    #[test]
    fn parse_returns_none_when_top_level_not_object() {
        let v = serde_json::json!([1, 2, 3]);
        assert!(parse_semantic_failure_report(&v).is_none());
    }

    #[test]
    fn dispatch_target_routes_setup_for_dependency_and_config() {
        let mut report = SemanticFailureReport {
            failure_kind: VerifierDiagnosticFailureKind::DependencyMissing,
            failure_clusters: Vec::new(),
            contract_conflict: ContractConflict {
                implementation: String::new(),
                test: String::new(),
                usage_docs: String::new(),
            },
            preferred_repair_role: ArtifactRole::Setup,
            repair_hypothesis: String::new(),
            confidence: 0.5,
        };
        assert_eq!(
            dispatch_target(&report),
            SemanticDispatchTarget::SetupRepair
        );

        report.failure_kind = VerifierDiagnosticFailureKind::ConfigOrVerifierError;
        assert_eq!(
            dispatch_target(&report),
            SemanticDispatchTarget::SetupRepair
        );
    }

    #[test]
    fn dispatch_target_routes_semantic_for_other_kinds() {
        for kind in [
            VerifierDiagnosticFailureKind::AssertionMismatch,
            VerifierDiagnosticFailureKind::RuntimeError,
            VerifierDiagnosticFailureKind::TestBug,
            VerifierDiagnosticFailureKind::CompileOrSyntaxError,
            VerifierDiagnosticFailureKind::LocalImportContractMismatch,
            VerifierDiagnosticFailureKind::Unknown,
        ] {
            let report = SemanticFailureReport {
                failure_kind: kind,
                failure_clusters: Vec::new(),
                contract_conflict: ContractConflict {
                    implementation: String::new(),
                    test: String::new(),
                    usage_docs: String::new(),
                },
                preferred_repair_role: ArtifactRole::Implementation,
                repair_hypothesis: String::new(),
                confidence: 0.5,
            };
            assert_eq!(
                dispatch_target(&report),
                SemanticDispatchTarget::SemanticRepair
            );
        }
    }

    #[test]
    fn schema_error_codes_are_stable() {
        assert_eq!(SchemaError::ConfidenceInvalid.code(), "confidence_invalid");
        assert_eq!(SchemaError::HypothesisTooLong.code(), "hypothesis_too_long");
        assert_eq!(
            SchemaError::AffectedCasesTooMany.code(),
            "affected_cases_too_many"
        );
        assert_eq!(
            SchemaError::ClusterKeyMalformed.code(),
            "cluster_key_malformed"
        );
    }

    #[test]
    fn parse_strips_control_chars_from_all_string_fields() {
        let v = serde_json::json!({
            "failure_kind": "assertion_mismatch",
            "confidence": 0.5,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": "hyp\x07with\x08bell",
            "contract_conflict": {
                "implementation": "impl\x01ctl",
                "test": "test\x02ctl",
                "usage_docs": "docs\x03ctl",
            },
            "failure_clusters": [
                {
                    "observed": "obs\x04",
                    "expected": "exp\x05",
                    "input_shape": "shp\x06",
                    "assertion_shape": "AssertEq",
                    "involved_artifacts": ["test"],
                    "affected_cases": ["case\x07one"],
                }
            ],
        });
        let parsed = parse_semantic_failure_report(&v).expect("parses");
        for s in [
            &parsed.repair_hypothesis,
            &parsed.contract_conflict.implementation,
            &parsed.contract_conflict.test,
            &parsed.contract_conflict.usage_docs,
            &parsed.failure_clusters[0].observed,
            &parsed.failure_clusters[0].expected,
            &parsed.failure_clusters[0].affected_cases[0],
        ] {
            assert!(
                !s.chars().any(|c| (c as u32) < 0x20),
                "control char leaked into parsed field: {s:?}",
            );
        }
    }

    #[test]
    fn is_already_sanitized_rejects_control_chars() {
        assert!(is_already_sanitized("ok ascii"));
        assert!(!is_already_sanitized("has \x07 bell"));
        assert!(!is_already_sanitized("has \x7f del"));
    }

    // ---- CB-002 (literal-to-shape normalization) ---- //

    #[test]
    fn normalize_to_shape_collapses_numbers() {
        assert_eq!(
            normalize_to_shape("got 12345 want 0"),
            "got <num> want <num>"
        );
    }

    #[test]
    fn normalize_to_shape_collapses_uuid() {
        let s = "trace-id 550e8400-e29b-41d4-a716-446655440000 failed";
        assert!(normalize_to_shape(s).contains("<uuid>"));
        assert!(!normalize_to_shape(s).contains("550e8400"));
    }

    #[test]
    fn normalize_to_shape_collapses_quoted_string() {
        assert_eq!(
            normalize_to_shape("expected \"alice\" got 'bob'"),
            "expected <str> got <str>"
        );
    }

    #[test]
    fn normalize_to_shape_collapses_path_segments() {
        let s = "GET /users/12345/orders/77 not found";
        let n = normalize_to_shape(s);
        assert!(n.contains("<path>"), "expected <path> in {n:?}");
        assert!(!n.contains("12345"), "raw number must not leak: {n:?}");
    }

    #[test]
    fn normalize_to_shape_collapses_long_token() {
        // AKIA-style key (24+ chars) collapses to <token>.
        let s = "AWS key AKIAIOSFODNN7EXAMPLEXYZ rejected";
        let n = normalize_to_shape(s);
        assert!(n.contains("<token>"), "expected <token> in {n:?}");
        assert!(!n.contains("AKIA"), "raw key prefix must not leak: {n:?}");
    }

    #[test]
    fn normalize_to_shape_truncates_long_raw_tail() {
        // No shape-collapsing match (each "ab " is short / has whitespace) →
        // falls to the 64-char truncate path.
        let raw: String = "ab ".repeat(100); // 300 chars total
        let n = normalize_to_shape(&raw);
        assert!(
            n.chars().count() <= 64 + 6,
            "len {} exceeded cap+marker",
            n.chars().count()
        );
    }

    #[test]
    fn cluster_key_secret_like_values_collapse_to_same_shape_cb002() {
        // CB-002 acceptance: different secret-like values that share the
        // same shape (long token, number, etc.) must produce identical
        // cluster keys, and the raw secret must NEVER appear in the hash
        // input (verified indirectly via two distinct secrets → same key).
        let a = build_failure_cluster_from_observation(
            "auth failed for AKIAIOSFODNN7EXAMPLEXYZ on /users/12345",
            "expected 200 got 401",
            "shape",
            "AssertEq",
            &role_set(&[ArtifactRole::Test]),
            Vec::new(),
        );
        let b = build_failure_cluster_from_observation(
            "auth failed for tok_abc123def456ghi789 on /users/67890",
            "expected 999 got 401",
            "shape",
            "AssertEq",
            &role_set(&[ArtifactRole::Test]),
            Vec::new(),
        );
        assert_eq!(
            a.cluster_key, b.cluster_key,
            "secret-like values must collapse to the same shape key"
        );
    }

    #[test]
    fn cluster_key_does_not_embed_raw_secret_cb002() {
        // CB-002: raw secret values must not appear in `cluster_key`'s hex.
        let secret = "AKIA0123456789ABCDEFGHIJ";
        let cluster = build_failure_cluster_from_observation(
            &format!("denied for {secret}"),
            "expected ok",
            "shape",
            "AssertEq",
            &role_set(&[ArtifactRole::Test]),
            Vec::new(),
        );
        // Hex key is 16 chars — verify it's not a substring of the secret
        // and the secret is not a substring of the hash inputs by
        // recomputing with a *different* secret of the same shape: keys
        // must agree (covered by the previous test) and neither key must
        // textually contain the secret.
        assert!(!cluster.cluster_key.as_str().contains(secret));
    }

    #[test]
    fn cluster_key_different_shapes_yield_different_keys_cb002() {
        // CB-002 sanity: shapes that genuinely differ must NOT collide.
        let assertion_failure = build_failure_cluster_from_observation(
            "assertion x == 1 failed",
            "got 2",
            "shape",
            "AssertEq",
            &role_set(&[ArtifactRole::Test]),
            Vec::new(),
        );
        let runtime_failure = build_failure_cluster_from_observation(
            "panic: divide by zero",
            "no panic expected",
            "shape",
            "Panic",
            &role_set(&[ArtifactRole::Test]),
            Vec::new(),
        );
        assert_ne!(assertion_failure.cluster_key, runtime_failure.cluster_key);
    }

    // ---- CB-017 A''' (Commit 2): RawClusterTargetCandidate schema ---- //

    #[test]
    fn cb017_failure_cluster_carries_proposed_target_candidates() {
        // The constructor accepts a Vec<RawClusterTargetCandidate> at parse
        // time. `admitted_cluster_targets` stays empty until the turn.rs
        // enrich step runs (Commit 3).
        let candidates = vec![
            RawClusterTargetCandidate {
                raw_path: "src/main.rs".to_string(),
                role_hint: Some(ArtifactRole::Implementation),
                reason: "fix null check".to_string(),
            },
            RawClusterTargetCandidate {
                raw_path: "tests/integration.rs".to_string(),
                role_hint: Some(ArtifactRole::Test),
                reason: "update fixture".to_string(),
            },
        ];
        let cluster = build_failure_cluster_from_observation(
            "observed",
            "expected",
            "shape",
            "AssertEq",
            &role_set(&[ArtifactRole::Implementation]),
            candidates.clone(),
        );
        assert_eq!(cluster.proposed_target_candidates, candidates);
        // Commit 2 invariant: admission is the turn.rs boundary's job.
        assert!(
            cluster.admitted_cluster_targets.is_empty(),
            "admitted_cluster_targets must be empty at parse time",
        );
    }

    #[test]
    fn cb017_parse_extracts_target_paths_from_cluster_json() {
        let v = serde_json::json!({
            "failure_kind": "assertion_mismatch",
            "confidence": 0.5,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": "h",
            "failure_clusters": [
                {
                    "observed": "o",
                    "expected": "e",
                    "input_shape": "s",
                    "assertion_shape": "AssertEq",
                    "involved_artifacts": ["implementation"],
                    "affected_cases": [],
                    "target_paths": [
                        {
                            "path": "src/main.rs",
                            "role": "implementation",
                            "reason": "fix null check",
                        }
                    ],
                }
            ],
        });
        let parsed = parse_semantic_failure_report(&v).expect("parses");
        assert_eq!(parsed.failure_clusters.len(), 1);
        let candidates = &parsed.failure_clusters[0].proposed_target_candidates;
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].raw_path, "src/main.rs");
        assert_eq!(candidates[0].role_hint, Some(ArtifactRole::Implementation));
        assert_eq!(candidates[0].reason, "fix null check");
        // Commit 2 invariant.
        assert!(
            parsed.failure_clusters[0]
                .admitted_cluster_targets
                .is_empty(),
            "parse must not populate admitted_cluster_targets",
        );
    }

    #[test]
    fn cb017_parse_truncates_target_paths_above_max() {
        // 9 entries → truncated to MAX_PROPOSED_TARGETS_PER_CLUSTER = 8.
        let entries: Vec<_> = (0..9)
            .map(|i| {
                serde_json::json!({
                    "path": format!("src/file_{}.rs", i),
                    "role": "implementation",
                    "reason": "r",
                })
            })
            .collect();
        let v = serde_json::json!({
            "failure_kind": "assertion_mismatch",
            "confidence": 0.5,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": "h",
            "failure_clusters": [
                {
                    "observed": "o",
                    "expected": "e",
                    "input_shape": "s",
                    "assertion_shape": "AssertEq",
                    "involved_artifacts": ["implementation"],
                    "affected_cases": [],
                    "target_paths": entries,
                }
            ],
        });
        let parsed = parse_semantic_failure_report(&v).expect("parses");
        assert_eq!(
            parsed.failure_clusters[0].proposed_target_candidates.len(),
            MAX_PROPOSED_TARGETS_PER_CLUSTER,
            "parse must truncate to MAX_PROPOSED_TARGETS_PER_CLUSTER",
        );
    }

    #[test]
    fn cb017_parse_role_hint_invalid_returns_none() {
        let v = serde_json::json!({
            "failure_kind": "assertion_mismatch",
            "confidence": 0.5,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": "h",
            "failure_clusters": [
                {
                    "observed": "o",
                    "expected": "e",
                    "input_shape": "s",
                    "assertion_shape": "AssertEq",
                    "involved_artifacts": ["implementation"],
                    "affected_cases": [],
                    "target_paths": [
                        {
                            "path": "src/main.rs",
                            "role": "garbage",
                            "reason": "r",
                        }
                    ],
                }
            ],
        });
        let parsed = parse_semantic_failure_report(&v).expect("parses");
        let cand = &parsed.failure_clusters[0].proposed_target_candidates[0];
        assert_eq!(cand.role_hint, None);
        // raw_path / reason are still sanitized + retained.
        assert_eq!(cand.raw_path, "src/main.rs");
        assert_eq!(cand.reason, "r");
    }

    #[test]
    fn cb017_parse_target_paths_absent_yields_empty() {
        // No `target_paths` field at all → proposed_target_candidates empty,
        // existing cluster fields preserved.
        let v = serde_json::json!({
            "failure_kind": "assertion_mismatch",
            "confidence": 0.5,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": "h",
            "failure_clusters": [
                {
                    "observed": "o",
                    "expected": "e",
                    "input_shape": "s",
                    "assertion_shape": "AssertEq",
                    "involved_artifacts": ["implementation"],
                    "affected_cases": [],
                }
            ],
        });
        let parsed = parse_semantic_failure_report(&v).expect("parses");
        assert!(
            parsed.failure_clusters[0]
                .proposed_target_candidates
                .is_empty(),
            "absent target_paths must yield empty candidates",
        );
        assert!(
            parsed.failure_clusters[0]
                .admitted_cluster_targets
                .is_empty(),
            "admitted_cluster_targets must be empty at parse time",
        );
    }

    #[test]
    fn cb017_cluster_key_unchanged_by_target_candidates() {
        // DR1-010: cluster_key hash inputs must not include
        // proposed_target_candidates. Two clusters with identical
        // observed/expected/role but different candidate lists must share a
        // cluster_key.
        let with_a = build_failure_cluster_from_observation(
            "o",
            "e",
            "s",
            "AssertEq",
            &role_set(&[ArtifactRole::Implementation]),
            vec![RawClusterTargetCandidate {
                raw_path: "src/a.rs".to_string(),
                role_hint: Some(ArtifactRole::Implementation),
                reason: "r".to_string(),
            }],
        );
        let with_b = build_failure_cluster_from_observation(
            "o",
            "e",
            "s",
            "AssertEq",
            &role_set(&[ArtifactRole::Implementation]),
            vec![RawClusterTargetCandidate {
                raw_path: "src/b.rs".to_string(),
                role_hint: Some(ArtifactRole::Test),
                reason: "different".to_string(),
            }],
        );
        assert_eq!(
            with_a.cluster_key, with_b.cluster_key,
            "cluster_key must be independent of proposed_target_candidates (DR1-010)",
        );
        let empty = build_failure_cluster_from_observation(
            "o",
            "e",
            "s",
            "AssertEq",
            &role_set(&[ArtifactRole::Implementation]),
            Vec::new(),
        );
        assert_eq!(
            with_a.cluster_key, empty.cluster_key,
            "cluster_key must be invariant when candidates are added",
        );
    }

    #[test]
    fn cb017_raw_path_sanitized_and_capped() {
        // A 250-char raw_path is truncated to MAX_RAW_PATH_CHARS (240) at
        // parse, via the SSOT sanitize entry (with trailing ellipsis ≤ +3).
        let long_path: String = "a".repeat(250);
        let v = serde_json::json!({
            "failure_kind": "assertion_mismatch",
            "confidence": 0.5,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": "h",
            "failure_clusters": [
                {
                    "observed": "o",
                    "expected": "e",
                    "input_shape": "s",
                    "assertion_shape": "AssertEq",
                    "involved_artifacts": ["implementation"],
                    "affected_cases": [],
                    "target_paths": [
                        {
                            "path": long_path,
                            "role": "implementation",
                            "reason": "r",
                        }
                    ],
                }
            ],
        });
        let parsed = parse_semantic_failure_report(&v).expect("parses");
        let cand = &parsed.failure_clusters[0].proposed_target_candidates[0];
        let n = cand.raw_path.chars().count();
        assert!(
            n <= MAX_RAW_PATH_CHARS + 3,
            "raw_path must be capped to MAX_RAW_PATH_CHARS (+ optional ellipsis); got {n}",
        );
        // No control chars leaked.
        assert!(!cand.raw_path.chars().any(|c| (c as u32) < 0x20));
    }

    // -- Phase G grep / structure tests (Issue #647 acceptance closure) -- //

    /// Helper: split the module source into the production prefix
    /// (everything before `#[cfg(test)]\nmod tests`). Phase G grep tests
    /// scrutinize production code only — test-only literals are exempt.
    fn production_source() -> &'static str {
        let source = include_str!("semantic_failure.rs");
        // Match the canonical opener of this file's test module.
        match source.find("#[cfg(test)]\nmod tests") {
            Some(idx) => &source[..idx],
            None => source,
        }
    }

    #[test]
    fn no_framework_literal_in_semantic_failure_module() {
        // S1-012 (拡張): new production code must not embed
        // framework-specific literals — those belong to evaluation
        // fixtures, not the semantic repair planner.
        let prod = production_source();
        for lit in &["\"422\"", "\"404\"", "/items/nonexistent", "FastAPI"] {
            assert!(
                !prod.contains(lit),
                "semantic_failure.rs production code must not contain framework literal {lit:?}",
            );
        }
    }

    #[test]
    fn no_unsafe_in_semantic_failure_module() {
        // DR4-003: no unsafe / FFI in new production code.
        let prod = production_source();
        assert!(
            !prod.contains("unsafe "),
            "semantic_failure.rs production code must not contain `unsafe `",
        );
        assert!(
            !prod.contains("extern \"C\""),
            "semantic_failure.rs production code must not declare FFI",
        );
    }
}
