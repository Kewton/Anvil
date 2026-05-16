//! ActionSummary -> photon PR #120 nested `/v1/summary/upsert` schema adapter
//! (Issue #604, DR2-005 / DR3-003).
//!
//! The local-flat `ActionSummary` (`src/session/case_photon_bridge.rs`) is a
//! JSONL-friendly representation used by Anvil's bridge / dry-run pipeline.
//! The photon sidecar PR #120 schema is a nested shape with stricter typed
//! `facts[]` / `avoid[]` / `next_hints[]` sub-objects and additional metadata
//! (`summary_level`, `quality_warnings`, `quality_check_status`).
//!
//! This module is the single point of conversion. Field names that do not
//! exist in photon PR #120 (notably `context_signature`, `_provenance`) are
//! intentionally NOT propagated to keep payloads minimal and forwards-
//! compatible.

use serde_json::{Value, json};

use crate::session::case_photon_bridge::ActionSummary;

/// Photon PR #120 `summary_level` constant. Anvil always promotes at the
/// task-instance level (one `CaseRecord` = one task), so this is a fixed
/// string rather than a parameter.
const SUMMARY_LEVEL: &str = "task";

/// Photon PR #120 default `quality_check_status` when no scrub findings are
/// pre-attached (the photon sidecar may relabel this after its own gate).
const QUALITY_CHECK_STATUS_DEFAULT: &str = "pending";

/// Convert a local `ActionSummary` to the photon PR #120 nested
/// `/v1/summary/upsert` body shape.
///
/// Output shape (only field names, see tests for canonical JSON):
///
/// ```text
/// {
///   "schema_version": "action-memory.v0.2",
///   "summary_id": "<sanitized>",
///   "session_id": <optional>,
///   "repo_id": <optional>,
///   "task_signature": <optional>,
///   "summary_level": "task",
///   "facts": [{"text", "evidence_ids", "confidence"}, ...],
///   "avoid": [{"action", "reason", "evidence_ids"}, ...],
///   "next_hints": [{"kind", "target", "reason", "confidence"}, ...],
///   "quality_warnings": [],
///   "quality_check_status": "pending"
/// }
/// ```
pub fn to_v2_upsert_summary(summary: &ActionSummary) -> Value {
    let facts: Vec<Value> = summary
        .facts
        .iter()
        .map(|f| {
            json!({
                "text": f.text,
                "evidence_ids": Vec::<String>::new(),
                "confidence": f.confidence,
            })
        })
        .collect();

    let avoid: Vec<Value> = summary
        .avoid
        .iter()
        .map(|a| {
            json!({
                "action": a.text,
                "reason": "anvil_case_avoid",
                "evidence_ids": Vec::<String>::new(),
            })
        })
        .collect();

    let next_hints: Vec<Value> = summary
        .next_hints
        .iter()
        .map(|h| {
            json!({
                "kind": h.kind,
                "target": h.target,
                "reason": "anvil_case_hint",
                "confidence": 0.6_f32,
            })
        })
        .collect();

    let mut out = json!({
        "schema_version": summary.schema_version,
        "summary_id": summary.summary_id,
        "summary_level": SUMMARY_LEVEL,
        "facts": facts,
        "avoid": avoid,
        "next_hints": next_hints,
        "quality_warnings": Vec::<String>::new(),
        "quality_check_status": QUALITY_CHECK_STATUS_DEFAULT,
    });

    // repo_id is required by local ActionSummary (non-Option String); skip
    // empty fallback strings so we forward only meaningful identifiers.
    if !summary.repo_id.trim().is_empty() {
        out["repo_id"] = Value::String(summary.repo_id.clone());
    }
    if !summary.task_signature.trim().is_empty() {
        out["task_signature"] = Value::String(summary.task_signature.clone());
    }
    if let Some(prov) = &summary.provenance
        && !prov.session_id.trim().is_empty()
    {
        out["session_id"] = Value::String(prov.session_id.clone());
    }

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::case_photon_bridge::{
        ACTION_SUMMARY_SCHEMA_VERSION, ActionSummary, Avoid, Fact, Hint, Provenance,
    };

    fn make_summary(
        facts: Vec<Fact>,
        avoid: Vec<Avoid>,
        next_hints: Vec<Hint>,
        provenance: Option<Provenance>,
    ) -> ActionSummary {
        ActionSummary {
            schema_version: ACTION_SUMMARY_SCHEMA_VERSION.to_string(),
            summary_id: "anvil-case-abc".to_string(),
            repo_id: "workspace:demo".to_string(),
            task_signature: "fix flaky test".to_string(),
            facts,
            avoid,
            next_hints,
            provenance,
        }
    }

    fn make_provenance() -> Provenance {
        Provenance {
            source: "anvil_case_record".to_string(),
            case_id: "abc".to_string(),
            session_id: "sess-1".to_string(),
            anvil_version: "0.6.0".to_string(),
            extracted_at: "2026-05-17T00:00:00Z".to_string(),
            confidence_prior: 1.0,
            verifier_active: true,
        }
    }

    #[test]
    fn empty_avoid_and_next_hints_produce_empty_arrays() {
        let s = make_summary(
            vec![Fact {
                text: "language_stack: rust".into(),
                confidence: 1.0,
            }],
            vec![],
            vec![],
            Some(make_provenance()),
        );
        let v = to_v2_upsert_summary(&s);
        assert_eq!(v["schema_version"], ACTION_SUMMARY_SCHEMA_VERSION);
        assert_eq!(v["summary_id"], "anvil-case-abc");
        assert_eq!(v["summary_level"], "task");
        assert_eq!(v["facts"].as_array().unwrap().len(), 1);
        assert_eq!(v["facts"][0]["text"], "language_stack: rust");
        assert_eq!(v["facts"][0]["evidence_ids"].as_array().unwrap().len(), 0);
        assert!((v["facts"][0]["confidence"].as_f64().unwrap() - 1.0).abs() < 1e-6);
        assert_eq!(v["avoid"].as_array().unwrap().len(), 0);
        assert_eq!(v["next_hints"].as_array().unwrap().len(), 0);
        assert_eq!(v["quality_warnings"].as_array().unwrap().len(), 0);
        assert_eq!(v["quality_check_status"], "pending");
        assert_eq!(v["session_id"], "sess-1");
        assert_eq!(v["repo_id"], "workspace:demo");
        assert_eq!(v["task_signature"], "fix flaky test");
    }

    #[test]
    fn populated_avoid_and_next_hints_are_mapped_to_nested_shape() {
        let s = make_summary(
            vec![Fact {
                text: "language_stack: rust, python".into(),
                confidence: 0.8,
            }],
            vec![Avoid {
                text: "initial_feedback: test_failure".into(),
                confidence: 0.8,
            }],
            vec![Hint {
                kind: "verify".to_string(),
                target: "cargo test --lib".to_string(),
            }],
            Some(make_provenance()),
        );
        let v = to_v2_upsert_summary(&s);

        let avoid = v["avoid"].as_array().unwrap();
        assert_eq!(avoid.len(), 1);
        assert_eq!(avoid[0]["action"], "initial_feedback: test_failure");
        assert_eq!(avoid[0]["reason"], "anvil_case_avoid");
        assert_eq!(avoid[0]["evidence_ids"].as_array().unwrap().len(), 0);

        let hints = v["next_hints"].as_array().unwrap();
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0]["kind"], "verify");
        assert_eq!(hints[0]["target"], "cargo test --lib");
        assert_eq!(hints[0]["reason"], "anvil_case_hint");
        assert!(hints[0]["confidence"].as_f64().is_some());
    }

    #[test]
    fn unknown_fields_are_not_propagated() {
        // local ActionSummary embeds `_provenance` (Provenance), but the photon
        // PR #120 schema does not have it. Verify it is dropped.
        let s = make_summary(vec![], vec![], vec![], Some(make_provenance()));
        let v = to_v2_upsert_summary(&s);
        let obj = v.as_object().expect("must be object");
        assert!(
            !obj.contains_key("_provenance"),
            "_provenance must not be forwarded; got keys {:?}",
            obj.keys().collect::<Vec<_>>(),
        );
        assert!(
            !obj.contains_key("context_signature"),
            "context_signature must not be present; got keys {:?}",
            obj.keys().collect::<Vec<_>>(),
        );
        assert!(
            !obj.contains_key("provenance"),
            "non-underscored provenance must also be absent",
        );
    }

    #[test]
    fn missing_provenance_omits_session_id() {
        let s = make_summary(vec![], vec![], vec![], None);
        let v = to_v2_upsert_summary(&s);
        assert!(
            v.as_object().unwrap().get("session_id").is_none(),
            "session_id should be omitted when provenance is None",
        );
    }

    #[test]
    fn schema_version_is_sourced_from_input_summary() {
        // Ensures the adapter does not hard-code the schema version (DR2-001
        // SSOT in case_photon_bridge::ACTION_SUMMARY_SCHEMA_VERSION).
        let mut s = make_summary(vec![], vec![], vec![], None);
        s.schema_version = "action-memory.vXX".to_string();
        let v = to_v2_upsert_summary(&s);
        assert_eq!(v["schema_version"], "action-memory.vXX");
    }

    #[test]
    fn full_canonical_shape_matches_expected() {
        // Pin the canonical JSON shape so future refactors break this test
        // before breaking photon PR #120 upstream contracts.
        let s = make_summary(
            vec![Fact {
                text: "language_stack: rust".into(),
                confidence: 1.0,
            }],
            vec![],
            vec![],
            Some(make_provenance()),
        );
        let v = to_v2_upsert_summary(&s);
        let obj = v.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "avoid",
                "facts",
                "next_hints",
                "quality_check_status",
                "quality_warnings",
                "repo_id",
                "schema_version",
                "session_id",
                "summary_id",
                "summary_level",
                "task_signature",
            ],
        );
    }
}
