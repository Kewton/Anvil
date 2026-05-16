//! Issue #594: photon seed provenance (read path).
//!
//! Extracts per-item `provenance` objects from `ContextPackResponse.context_pack.items[]`
//! and surfaces them through `/photon-why` slash command and the
//! `agent.photon_context_pack.completed` event payload.
//!
//! ## Layer policy (DR3-002)
//! This module is part of the `photon` layer and MUST NOT import any types from
//! the `agent` or `session` layers. The single SSOT helper from `session` we
//! pull in (`mask_secrets`) follows the same precedent as `prompt.rs:25`.
//!
//! ## Security pipeline (5 layers, fail-closed per field)
//! For each string field (source_id / source_repo / version / created_at):
//!   1. byte cap (per-field constant)
//!   2. ASCII control characters (`char::is_ascii_control`)
//!   3. bidi / format control characters (`prompt::is_bidi_control` SSOT)
//!   4. `mask_secrets(s) != s` (potential secret leak)
//!   5. `prompt::contains_secret_word(s)` (token/secret/password/etc)
//!
//! ## CB-001 / CB-002 ordering (Issue #594 review fix)
//! For the whole provenance object the pipeline runs strictly in this order:
//!
//! 1. Type/null guard (returns `"missing"` / `"invalid"`).
//! 2. Serialized byte cap (`MAX_PROVENANCE_OBJECT_BYTES`) — evaluated against
//!    the **borrowed** input value, no clone happens before this gate.
//! 3. Bounded clone of allowlisted top-level fields only (DR4-002).
//! 4. **Field-level 5-layer sanitize on the original raw strings** so bare
//!    AWS / GitHub / OpenAI tokens are detected by `mask_secrets(raw) != raw`
//!    and the field is dropped (CB-002).
//! 5. Projection into `SeedProvenanceSummary` strings.
//!
//! Step 4 happens inside `build_summary_from_value`; the projection into
//! `SeedProvenanceSummary` strings is what flows out of the module.
//!
//! Note: `mask_payload_inplace` is intentionally NOT applied here. The
//! current allowlist contains no secret-like keys (the post-projection
//! pass would be a no-op), and running it pre-pipeline would erase the
//! `mask_secrets(raw) != raw` signal used to drop bare tokens (CB-002).
//! Future allowlist additions must update both this comment and the
//! `pv_00_event_keys_are_not_secret_like` invariant.
//!
//! ## Invariant
//! `enumerate_admitted_items_with_provenance` (in `prompt.rs`) MUST return one
//! `AdmittedItemView` per `items_adopted`. Missing / malformed / oversized
//! provenance values produce a placeholder `SeedProvenanceSummary` row with
//! `source = "unknown"` and `provenance_status` describing the cause; the row
//! count never drops below `items_adopted` (PV-01).

use serde::Serialize;
use serde_json::Value;

use crate::photon::prompt::{
    MAX_BLOCKED_SUMMARY_ID_BYTES, contains_secret_word, is_bidi_control, sanitize_summary_id,
};
use crate::session::feedback::mask_secrets;

// ---------------------------------------------------------------------------
// Public constants (SSOT)
// ---------------------------------------------------------------------------

/// Maximum serialized JSON bytes of a single provenance object before it is
/// treated as oversized and replaced with a placeholder row.
pub const MAX_PROVENANCE_OBJECT_BYTES: usize = 4096;

/// Source-ID byte cap. Independent from `MAX_BLOCKED_SUMMARY_ID_BYTES` even
/// though the numeric value coincides today — these are different categories
/// (case_id allowlist vs URL/path slug) and may diverge in future Issues.
pub const MAX_PROVENANCE_SOURCE_ID_BYTES: usize = 256;

/// Maximum number of `correlated_case_ids` retained per item before truncation.
pub const MAX_CORRELATED_CASE_IDS: usize = 8;

/// Version-field byte cap (anvil_version / photon_version), sized for semver.
pub const MAX_PROVENANCE_VERSION_BYTES: usize = 64;

/// `created_at` byte cap (RFC 3339 with timezone + microseconds).
pub const MAX_PROVENANCE_CREATED_AT_BYTES: usize = 64;

/// Allowlist of normalized `source` values. Anything not matching here
/// (case-insensitively) is fail-open mapped to `"unknown"`.
pub const ALLOWED_PROVENANCE_SOURCE_VALUES: &[&str] = &[
    "human_handcrafted",
    "anvil_case_record",
    "user_slash_command",
    "imported_from_repo",
    "generated_by_llm",
    "unknown",
];

/// Allowlist of `trust_tier` values. Anything else yields `None`.
pub const ALLOWED_TRUST_TIER_VALUES: &[&str] =
    &["human_reviewed", "auto_extracted", "experimental"];

// ---------------------------------------------------------------------------
// Public value objects
// ---------------------------------------------------------------------------

/// Per-item provenance summary surfaced in `/photon-why` and the
/// `agent.photon_context_pack.completed` event payload (Serialize-only).
///
/// Note: `source` is `&'static str` rather than `Option` because every
/// admitted item produces a placeholder row when provenance is absent
/// (fail-open), maintaining the invariant
/// `injected_seed_provenance_summary.len() == RenderStats.items_adopted`.
#[derive(Debug, Clone, Serialize)]
pub struct SeedProvenanceSummary {
    /// Summary item id, already normalized through `sanitize_summary_id`.
    pub summary_id: Option<String>,
    /// Normalized provenance source label (allowlist value, defaults to `"unknown"`).
    pub source: &'static str,
    /// Original raw source id, sanitized through the 5-layer pipeline (256-byte cap).
    pub source_id: Option<String>,
    /// Originating repository slug (sanitized, 256-byte cap).
    pub source_repo: Option<String>,
    /// Normalized trust tier (allowlist value, `None` when missing or invalid).
    pub trust_tier: Option<&'static str>,
    /// Anvil version string (sanitized, 64-byte cap).
    pub anvil_version: Option<String>,
    /// Photon sidecar version string (sanitized, 64-byte cap).
    pub photon_version: Option<String>,
    /// Creation timestamp (sanitized, 64-byte cap, RFC 3339 shape).
    pub created_at: Option<String>,
    /// Correlated case ids (each entry through `sanitize_summary_id`, up to 8).
    pub correlated_case_ids: Vec<String>,
    /// Provenance lifecycle status. One of:
    /// `"present"` / `"missing"` / `"invalid"` / `"oversized"` / `"sanitized"`.
    pub provenance_status: &'static str,
}

impl Default for SeedProvenanceSummary {
    fn default() -> Self {
        Self {
            summary_id: None,
            source: "unknown",
            source_id: None,
            source_repo: None,
            trust_tier: None,
            anvil_version: None,
            photon_version: None,
            created_at: None,
            correlated_case_ids: Vec::new(),
            provenance_status: "missing",
        }
    }
}

/// Outcome of a single-pass `sanitize_provenance_value` call.
///
/// `value` is the bounded clone *before* per-field secret masking, so the
/// caller (`build_summary_from_value`) can apply the 5-layer field-level
/// pipeline against the original raw strings and detect bare tokens
/// (CB-002). `mask_payload_inplace` is intentionally NOT applied here —
/// running it pre-pipeline would replace bare tokens with `"***"` and
/// erase the `mask_secrets(raw) != raw` signal used to drop them. The
/// current allowlist contains no secret-like keys (post-projection mask
/// would be a no-op); the 5-layer field-level pipeline is the secret
/// defence. See the module-level "CB-001 / CB-002 ordering" note.
///
/// `status` is the same lifecycle vocabulary used in `SeedProvenanceSummary`.
#[derive(Debug, Clone)]
pub struct ProvenanceSanitizeOutcome {
    /// Sanitized provenance value (None when fully dropped).
    pub value: Option<Value>,
    /// Status: `"present"` / `"missing"` / `"invalid"` / `"oversized"` / `"sanitized"`.
    pub status: &'static str,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Extract a `SeedProvenanceSummary` row from an admitted item.
///
/// Always returns a row (even when `raw_provenance` is `None`), preserving the
/// `views.len() == items_adopted` invariant. `summary_id` is normalized
/// through `sanitize_summary_id` (rejecting any value that fails the
/// case-id allowlist).
pub fn extract_seed_provenance(
    summary_id: Option<&str>,
    raw_provenance: Option<&Value>,
) -> SeedProvenanceSummary {
    let normalized_summary_id = summary_id.and_then(sanitize_summary_id);

    let outcome = sanitize_provenance_value(raw_provenance);
    match outcome.value {
        None => SeedProvenanceSummary {
            summary_id: normalized_summary_id,
            provenance_status: outcome.status,
            ..SeedProvenanceSummary::default()
        },
        Some(value) => {
            // CB-002: the 5-layer field-level pipeline below runs on the
            // bounded clone's raw strings so bare token leaves (AWS / GitHub
            // / OpenAI / JWT) are detected by `mask_secrets(raw) != raw` and
            // dropped via `sanitize_provenance_string`. `mask_payload_inplace`
            // must not run before this point — it would replace bare tokens
            // with `"***"` and erase the diff signal.
            //
            // The current allowlist (source / source_id / source_repo /
            // trust_tier / anvil_version / photon_version / created_at /
            // correlated_case_ids) contains no secret-like keys, so a
            // post-projection `mask_payload_inplace` would be a no-op. The
            // 5-layer pipeline is itself the secret defence. The single
            // source of truth for that invariant lives in `pv_00_*` and
            // CB-002 regression coverage.
            build_summary_from_value(normalized_summary_id, &value, outcome.status)
        }
    }
}

/// Sanitize a raw provenance `Value` by:
///   1. Detecting missing input (`None` / `Value::Null`) → `"missing"`.
///   2. Enforcing the serialized byte cap → `"oversized"` (CB-001: evaluated
///      against the borrowed input; no clone happens before this gate).
///   3. Bounded-cloning the allowlisted top-level fields only.
///
/// Returns the bounded clone alongside a status. Per-field 5-layer
/// sanitization (including `mask_secrets(raw) != raw` detection, CB-002)
/// happens later inside `build_summary_from_value` so that a single bad
/// field can be dropped without losing the rest of the provenance row.
/// `mask_payload_inplace` is intentionally NOT applied here — running it
/// pre-pipeline would convert bare tokens to `"***"` and erase the
/// `mask_secrets(raw) != raw` signal (CB-002). The current allowlist
/// contains no secret-like keys; the 5-layer pipeline is the SSOT secret
/// defence.
pub fn sanitize_provenance_value(value: Option<&Value>) -> ProvenanceSanitizeOutcome {
    let Some(value) = value else {
        return ProvenanceSanitizeOutcome {
            value: None,
            status: "missing",
        };
    };
    if value.is_null() {
        return ProvenanceSanitizeOutcome {
            value: None,
            status: "missing",
        };
    }
    if !value.is_object() {
        return ProvenanceSanitizeOutcome {
            value: None,
            status: "invalid",
        };
    }
    // CB-001: evaluate the serialized byte cap against the borrowed `value`
    // *before* any clone is taken. `serde_json::to_string(value)` produces a
    // throwaway String of bounded size; we never copy raw Value payloads
    // (e.g. unknown nested fields) into the heap before this gate fires.
    let serialized_len = serde_json::to_string(value)
        .map(|s| s.len())
        .unwrap_or(usize::MAX);
    if serialized_len > MAX_PROVENANCE_OBJECT_BYTES {
        return ProvenanceSanitizeOutcome {
            value: None,
            status: "oversized",
        };
    }

    // Bounded clone — copy only allowlisted top-level keys to keep the
    // recursive mask surface area small (DR4-002). At this point we know the
    // serialized size is within `MAX_PROVENANCE_OBJECT_BYTES`, so the clone
    // is strictly bounded by that cap.
    let obj = value.as_object().expect("checked above");
    let mut cloned = serde_json::Map::new();
    let mut had_unknown_field = false;
    for (k, v) in obj.iter() {
        match k.as_str() {
            "source"
            | "source_id"
            | "source_repo"
            | "trust_tier"
            | "anvil_version"
            | "photon_version"
            | "created_at"
            | "correlated_case_ids" => {
                cloned.insert(k.clone(), v.clone());
            }
            _ => {
                // Unknown fields are dropped; signal sanitized status.
                had_unknown_field = true;
            }
        }
    }
    // CB-002: we intentionally do NOT call `mask_payload_inplace` here.
    // The 5-layer field-level pipeline in `build_summary_from_value` must
    // run on the original raw strings to detect bare tokens
    // (`mask_secrets(raw) != raw`). The defensive `mask_payload_inplace`
    // pass is moved to the end of `extract_seed_provenance`.
    let cloned_value = Value::Object(cloned);

    let status = if had_unknown_field {
        "sanitized"
    } else {
        "present"
    };
    ProvenanceSanitizeOutcome {
        value: Some(cloned_value),
        status,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Translate a bounded-cloned provenance `Value::Object` into a
/// `SeedProvenanceSummary`. Each string field is run through the 5-layer
/// sanitize pipeline (`sanitize_provenance_string`); any field that fails is
/// dropped and the status is downgraded to `"sanitized"`.
fn build_summary_from_value(
    summary_id: Option<String>,
    value: &Value,
    base_status: &'static str,
) -> SeedProvenanceSummary {
    let obj = match value.as_object() {
        Some(o) => o,
        None => {
            return SeedProvenanceSummary {
                summary_id,
                provenance_status: "invalid",
                ..SeedProvenanceSummary::default()
            };
        }
    };

    let mut field_was_dropped = false;

    let (source, source_dropped) = match obj.get("source").and_then(|v| v.as_str()) {
        Some(raw) => (parse_provenance_source(raw), false),
        None => ("unknown", obj.contains_key("source")),
    };
    if source_dropped {
        field_was_dropped = true;
    }

    let (source_id, dropped) =
        sanitize_optional_string(obj.get("source_id"), MAX_PROVENANCE_SOURCE_ID_BYTES);
    if dropped {
        field_was_dropped = true;
    }
    let (source_repo, dropped) =
        sanitize_optional_string(obj.get("source_repo"), MAX_PROVENANCE_SOURCE_ID_BYTES);
    if dropped {
        field_was_dropped = true;
    }
    let (anvil_version, dropped) =
        sanitize_optional_string(obj.get("anvil_version"), MAX_PROVENANCE_VERSION_BYTES);
    if dropped {
        field_was_dropped = true;
    }
    let (photon_version, dropped) =
        sanitize_optional_string(obj.get("photon_version"), MAX_PROVENANCE_VERSION_BYTES);
    if dropped {
        field_was_dropped = true;
    }
    let (created_at, dropped) =
        sanitize_optional_string(obj.get("created_at"), MAX_PROVENANCE_CREATED_AT_BYTES);
    if dropped {
        field_was_dropped = true;
    }

    let trust_tier = match obj.get("trust_tier").and_then(|v| v.as_str()) {
        Some(raw) => {
            let parsed = parse_trust_tier(raw);
            if parsed.is_none() {
                field_was_dropped = true;
            }
            parsed
        }
        None => None,
    };

    let correlated_case_ids = match obj.get("correlated_case_ids") {
        Some(Value::Array(arr)) => {
            let mut out = Vec::new();
            for entry in arr.iter() {
                if out.len() >= MAX_CORRELATED_CASE_IDS {
                    field_was_dropped = true;
                    break;
                }
                match entry.as_str().and_then(sanitize_summary_id) {
                    Some(cleaned) => out.push(cleaned),
                    None => field_was_dropped = true,
                }
            }
            // Defensive: cap output even if loop logic ever diverges.
            out.truncate(MAX_CORRELATED_CASE_IDS);
            out
        }
        Some(_) => {
            field_was_dropped = true;
            Vec::new()
        }
        None => Vec::new(),
    };

    let provenance_status = if field_was_dropped {
        "sanitized"
    } else {
        base_status
    };

    SeedProvenanceSummary {
        summary_id,
        source,
        source_id,
        source_repo,
        trust_tier,
        anvil_version,
        photon_version,
        created_at,
        correlated_case_ids,
        provenance_status,
    }
}

/// Sanitize an optional string field through the 5-layer pipeline.
///
/// Returns `(value, was_dropped)` so the caller can record a "sanitized"
/// status when the original key was present but rejected.
fn sanitize_optional_string(field: Option<&Value>, byte_cap: usize) -> (Option<String>, bool) {
    match field {
        Some(Value::String(s)) => match sanitize_provenance_string(s, byte_cap) {
            Some(clean) => (Some(clean), false),
            None => (None, true),
        },
        Some(Value::Null) | None => (None, false),
        Some(_) => (None, true),
    }
}

/// 5-layer fail-closed sanitizer for a single string field.
///
/// All layers reuse SSOT helpers (DR1-001):
///   - Layer 3 calls `prompt::is_bidi_control` (`pub(crate)`).
///   - Layer 4 calls `session::feedback::mask_secrets` (existing prompt.rs import).
///   - Layer 5 calls `prompt::contains_secret_word` (`pub(crate)`).
pub(crate) fn sanitize_provenance_string(raw: &str, byte_cap: usize) -> Option<String> {
    if raw.len() > byte_cap {
        return None;
    }
    if raw.chars().any(|c| c.is_ascii_control()) {
        return None;
    }
    if raw.chars().any(is_bidi_control) {
        return None;
    }
    let masked = mask_secrets(raw);
    if masked != raw {
        return None;
    }
    if contains_secret_word(raw) {
        return None;
    }
    Some(raw.to_string())
}

/// Map a raw `source` value to its normalized allowlist entry. Unknown
/// values fail-open to `"unknown"` (DR3-001).
pub(crate) fn parse_provenance_source(raw: &str) -> &'static str {
    for &allowed in ALLOWED_PROVENANCE_SOURCE_VALUES {
        if allowed.eq_ignore_ascii_case(raw) {
            return allowed;
        }
    }
    "unknown"
}

/// Map a raw `trust_tier` value to its normalized allowlist entry, or `None`.
pub(crate) fn parse_trust_tier(raw: &str) -> Option<&'static str> {
    ALLOWED_TRUST_TIER_VALUES
        .iter()
        .find(|&&allowed| allowed.eq_ignore_ascii_case(raw))
        .copied()
}

// Compile-time sanity: keep the source-id cap in lock-step with
// `sanitize_summary_id`'s byte cap. This is *not* an alias (see policy comment
// on `MAX_PROVENANCE_SOURCE_ID_BYTES`); it is a static-assert documenting that
// they happen to coincide today.
const _: () = assert!(MAX_PROVENANCE_SOURCE_ID_BYTES == MAX_BLOCKED_SUMMARY_ID_BYTES);

// ---------------------------------------------------------------------------
// Module unit tests (DR3-001, includes PV-00 secret-like key naming)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // PV-00: every new event payload key the design adds must be classified as
    // non-secret-like by `is_secret_like_key`. Lives here (not in
    // `tests/photon_provenance_smoke.rs`) because `is_secret_like_key` is
    // `pub(crate)`.
    #[test]
    fn pv_00_event_keys_are_not_secret_like() {
        use crate::logging::is_secret_like_key;
        for key in [
            "injected_seed_provenance_summary",
            "summary_id",
            "source",
            "source_id",
            "source_repo",
            "trust_tier",
            "provenance_status",
            "anvil_version",
            "photon_version",
            "created_at",
            "correlated_case_ids",
        ] {
            assert!(
                !is_secret_like_key(key),
                "key {key} must not be flagged as secret-like"
            );
        }
    }

    // ---- sanitize_provenance_value -----------------------------------------

    #[test]
    fn sanitize_none_returns_missing() {
        let out = sanitize_provenance_value(None);
        assert_eq!(out.status, "missing");
        assert!(out.value.is_none());
    }

    #[test]
    fn sanitize_null_returns_missing() {
        let out = sanitize_provenance_value(Some(&Value::Null));
        assert_eq!(out.status, "missing");
        assert!(out.value.is_none());
    }

    #[test]
    fn sanitize_non_object_returns_invalid() {
        let out = sanitize_provenance_value(Some(&json!("a string")));
        assert_eq!(out.status, "invalid");
        assert!(out.value.is_none());
    }

    #[test]
    fn sanitize_oversized_returns_oversized() {
        // Build a 5000-byte string inside a provenance object so the
        // serialized form exceeds `MAX_PROVENANCE_OBJECT_BYTES = 4096`.
        let huge = "x".repeat(5000);
        let val = json!({ "source": "anvil_case_record", "source_repo": huge });
        let out = sanitize_provenance_value(Some(&val));
        assert_eq!(out.status, "oversized");
        assert!(out.value.is_none());
    }

    #[test]
    fn sanitize_unknown_field_drops_and_marks_sanitized() {
        let val = json!({
            "source": "anvil_case_record",
            "trust_tier": "auto_extracted",
            "experimental_field": "should be dropped",
        });
        let out = sanitize_provenance_value(Some(&val));
        assert_eq!(out.status, "sanitized");
        let cloned = out.value.expect("sanitized value present");
        assert!(cloned.get("experimental_field").is_none());
        assert_eq!(
            cloned.get("source").and_then(|v| v.as_str()),
            Some("anvil_case_record")
        );
    }

    // ---- extract_seed_provenance -------------------------------------------

    #[test]
    fn extract_returns_placeholder_when_missing() {
        let summary = extract_seed_provenance(Some("case_abc"), None);
        assert_eq!(summary.source, "unknown");
        assert_eq!(summary.provenance_status, "missing");
        assert_eq!(summary.summary_id.as_deref(), Some("case_abc"));
    }

    #[test]
    fn extract_normalizes_known_source() {
        let prov = json!({
            "source": "ANVIL_CASE_RECORD",
            "trust_tier": "Auto_Extracted",
            "source_id": "case_019dde7d",
            "created_at": "2026-05-12T15:30:00Z",
        });
        let summary = extract_seed_provenance(Some("case_abc"), Some(&prov));
        assert_eq!(summary.source, "anvil_case_record");
        assert_eq!(summary.trust_tier, Some("auto_extracted"));
        assert_eq!(summary.source_id.as_deref(), Some("case_019dde7d"));
        assert_eq!(summary.created_at.as_deref(), Some("2026-05-12T15:30:00Z"));
        // Present + no unknown field + no dropped field → "present"
        assert_eq!(summary.provenance_status, "present");
    }

    #[test]
    fn extract_unknown_source_falls_back_to_unknown() {
        let prov = json!({"source": "future_kind", "trust_tier": "experimental"});
        let summary = extract_seed_provenance(None, Some(&prov));
        assert_eq!(summary.source, "unknown");
        assert_eq!(summary.trust_tier, Some("experimental"));
    }

    #[test]
    fn extract_oversized_source_id_drops_field() {
        let huge_id = "x".repeat(MAX_PROVENANCE_SOURCE_ID_BYTES + 1);
        let prov = json!({"source": "anvil_case_record", "source_id": huge_id});
        let summary = extract_seed_provenance(None, Some(&prov));
        assert!(summary.source_id.is_none());
        assert_eq!(summary.provenance_status, "sanitized");
    }

    #[test]
    fn extract_secret_like_source_id_is_dropped() {
        // mask_secrets will alter strings containing `token=` form payloads.
        let prov = json!({
            "source": "anvil_case_record",
            "source_id": "token=abcdef1234567890abcdef1234567890",
        });
        let summary = extract_seed_provenance(None, Some(&prov));
        assert!(
            summary.source_id.is_none(),
            "secret-like source_id must be dropped"
        );
        assert_eq!(summary.provenance_status, "sanitized");
    }

    /// CB-002 (pv_19): bare AWS / GitHub / OpenAI / JWT tokens dropped into
    /// `source_id` must be detected by `mask_secrets(raw) != raw` and the
    /// field must be dropped. The bug under CB-002 was that
    /// `mask_payload_inplace` ran before the field-level check and
    /// converted bare tokens to `"***"`, which then passed
    /// `mask_secrets("***") == "***"` and was kept as `source_id="***"`.
    #[test]
    fn pv_19_bare_token_in_source_id_drops_field() {
        // AWS access key (`AKIA…` prefix).
        let aws = json!({
            "source": "anvil_case_record",
            "source_id": "AKIAIOSFODNN7EXAMPLE",
        });
        let s = extract_seed_provenance(None, Some(&aws));
        assert!(s.source_id.is_none(), "bare AWS access key must be dropped");
        assert_eq!(s.provenance_status, "sanitized");

        // GitHub personal access token (`ghp_…`).
        let gh = json!({
            "source": "anvil_case_record",
            "source_id": "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
        });
        let s = extract_seed_provenance(None, Some(&gh));
        assert!(s.source_id.is_none(), "bare GitHub PAT must be dropped");
        assert_eq!(s.provenance_status, "sanitized");

        // OpenAI API key (`sk-…`).
        let oa = json!({
            "source": "anvil_case_record",
            "source_id": "sk-proj-abcdefghijklmnopqrstuvwxyz0123456789",
        });
        let s = extract_seed_provenance(None, Some(&oa));
        assert!(s.source_id.is_none(), "bare OpenAI key must be dropped");
        assert_eq!(s.provenance_status, "sanitized");

        // JWT (`eyJ…`).
        let jwt = json!({
            "source": "anvil_case_record",
            "source_id": "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c",
        });
        let s = extract_seed_provenance(None, Some(&jwt));
        assert!(s.source_id.is_none(), "bare JWT must be dropped");
        assert_eq!(s.provenance_status, "sanitized");
    }

    /// CB-002 regression: a bare token must NOT be returned with the value
    /// `"***"` (which would happen if `mask_payload_inplace` ran before the
    /// field-level check). The `source_id` field must be fully absent.
    #[test]
    fn pv_19b_bare_token_must_not_round_trip_as_stars() {
        let prov = json!({
            "source": "anvil_case_record",
            "source_id": "AKIAIOSFODNN7EXAMPLE",
        });
        let s = extract_seed_provenance(None, Some(&prov));
        assert_ne!(
            s.source_id.as_deref(),
            Some("***"),
            "bare token must not survive as `***`; it must be dropped entirely"
        );
        assert!(s.source_id.is_none());
    }

    #[test]
    fn extract_correlated_case_ids_capped() {
        let ids: Vec<String> = (0..MAX_CORRELATED_CASE_IDS + 5)
            .map(|i| format!("case_{i:032}"))
            .collect();
        let prov = json!({"source": "anvil_case_record", "correlated_case_ids": ids});
        let summary = extract_seed_provenance(None, Some(&prov));
        assert_eq!(summary.correlated_case_ids.len(), MAX_CORRELATED_CASE_IDS);
        assert_eq!(summary.provenance_status, "sanitized");
    }

    #[test]
    fn extract_drops_bidi_in_source_repo() {
        // Insert a zero-width space (U+200B) — a bidi-class char that
        // `sanitize_provenance_string` must reject.
        let prov = json!({
            "source": "anvil_case_record",
            "source_repo": "github.com/foo/\u{200b}bar",
        });
        let summary = extract_seed_provenance(None, Some(&prov));
        assert!(summary.source_repo.is_none());
        assert_eq!(summary.provenance_status, "sanitized");
    }

    #[test]
    fn parse_provenance_source_accepts_all_known() {
        for &v in ALLOWED_PROVENANCE_SOURCE_VALUES {
            assert_eq!(parse_provenance_source(v), v);
        }
        assert_eq!(
            parse_provenance_source("ANVIL_CASE_RECORD"),
            "anvil_case_record"
        );
        assert_eq!(parse_provenance_source("nope"), "unknown");
    }

    #[test]
    fn parse_trust_tier_accepts_all_known() {
        for &v in ALLOWED_TRUST_TIER_VALUES {
            assert_eq!(parse_trust_tier(v), Some(v));
        }
        assert_eq!(parse_trust_tier("Auto_Extracted"), Some("auto_extracted"));
        assert!(parse_trust_tier("nope").is_none());
    }
}
