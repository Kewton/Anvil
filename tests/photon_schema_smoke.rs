use anvil::photon::{
    ContextPackRequest, ContextPackResponse, EvaluateRequest, EvaluateResponse, HealthResponse,
};
use serde::{Serialize, de::DeserializeOwned};

fn assert_newtype_roundtrip<T>(constructed: T)
where
    T: Serialize + DeserializeOwned,
{
    let json = serde_json::to_string(&constructed).unwrap();
    let restored: T = serde_json::from_str(&json).unwrap();
    let re_json = serde_json::to_string(&restored).unwrap();
    assert_eq!(json, re_json, "roundtrip must preserve JSON representation");
}

// S1: HealthResponse ok=true roundtrip
#[test]
fn s1_health_response_ok_true_roundtrip() {
    let original = HealthResponse { ok: true };
    let json = serde_json::to_string(&original).unwrap();
    let restored: HealthResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(original.ok, restored.ok);
}

// S2: HealthResponse ok=false roundtrip
#[test]
fn s2_health_response_ok_false_roundtrip() {
    let original = HealthResponse { ok: false };
    let json = serde_json::to_string(&original).unwrap();
    let restored: HealthResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(original.ok, restored.ok);
}

// S3: HealthResponse JSON representation is {"ok":true}
#[test]
fn s3_health_response_json_repr() {
    let resp = HealthResponse { ok: true };
    let json = serde_json::to_string(&resp).unwrap();
    assert_eq!(json, r#"{"ok":true}"#);
}

// S4: ContextPackRequest empty object roundtrip
#[test]
fn s4_context_pack_request_empty_object_roundtrip() {
    assert_newtype_roundtrip(ContextPackRequest(serde_json::json!({})));
}

// S5: ContextPackRequest nested value roundtrip
#[test]
fn s5_context_pack_request_nested_value_roundtrip() {
    assert_newtype_roundtrip(ContextPackRequest(serde_json::json!({
        "schema_version": 1,
        "task": "implement feature",
        "touched_files": ["src/main.rs", "src/lib.rs"]
    })));
}

// S6: ContextPackResponse empty object roundtrip
#[test]
fn s6_context_pack_response_empty_object_roundtrip() {
    assert_newtype_roundtrip(ContextPackResponse(serde_json::json!({})));
}

// S7: ContextPackResponse with items array roundtrip
#[test]
fn s7_context_pack_response_items_array_roundtrip() {
    assert_newtype_roundtrip(ContextPackResponse(serde_json::json!({
        "items": [
            {"kind": "summary", "summary": "relevant context", "source": "repo"},
            {"kind": "log", "summary": "log entry", "source": "history"}
        ]
    })));
}

// S8: EvaluateRequest roundtrip (API contract pin)
#[test]
fn s8_evaluate_request_roundtrip() {
    assert_newtype_roundtrip(EvaluateRequest(serde_json::json!({
        "session_id": "abc123",
        "turn_idx": 1,
        "context_pack_id": "cp_xyz"
    })));
}

// S9: EvaluateResponse with admission_decision roundtrip
#[test]
fn s9_evaluate_response_admitted_roundtrip() {
    assert_newtype_roundtrip(EvaluateResponse(serde_json::json!({
        "admission_decision": "admitted",
        "retry_summary": null
    })));
}

// S10: EvaluateResponse with warnings array roundtrip
#[test]
fn s10_evaluate_response_warnings_roundtrip() {
    assert_newtype_roundtrip(EvaluateResponse(serde_json::json!({
        "admission_decision": "rejected",
        "warnings": ["potential prompt injection", "large context"],
        "retry_summary": "retry after reducing context"
    })));
}

// S11 (Issue #594): ContextPackResponse preserves nested provenance fields
// (including unknown sub-fields) through serde roundtrip.
//
// The newtype around `serde_json::Value` should pass arbitrary extra fields
// inside `items[i].provenance` through unchanged so that future photon-side
// schema additions do not require coordinated Anvil-side releases.
#[test]
fn s11_context_pack_response_preserves_items_provenance_nested() {
    let raw = serde_json::json!({
        "context_pack": {
            "items": [{
                "kind": "summary",
                "id": "case_abc123",
                "text": "use bevy 0.13 API",
                "provenance": {
                    "source": "anvil_case_record",
                    "source_id": "case_019dde7d",
                    "trust_tier": "auto_extracted",
                    "correlated_case_ids": ["case_001", "case_002"],
                    "nested_extra": { "future_field": 42 }
                }
            }]
        }
    });
    assert_newtype_roundtrip(ContextPackResponse(raw));
}
