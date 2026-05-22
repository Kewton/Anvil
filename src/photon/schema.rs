use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPackRequest(pub serde_json::Value);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPackResponse(pub serde_json::Value);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluateRequest(pub serde_json::Value);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluateResponse(pub serde_json::Value);

// ---------------------------------------------------------------------------
// /v1/summary/upsert (Issue #604, photon-action-memory PR #119/#120)
// ---------------------------------------------------------------------------
//
// DR1-013 / DR3-002: photon layer never imports session-layer `ActionSummary`.
// The agent layer converts a local `ActionSummary` to a nested
// `serde_json::Value` via `to_v2_upsert_summary` before calling
// `PhotonClient::upsert_action_summary`.
//
// DR2-001: `schema_version` is a `String` whose value is the session-layer
// SSOT `ACTION_SUMMARY_SCHEMA_VERSION` (= `"action-memory.v0.2"`). A separate
// `DEFAULT_SCHEMA_VERSION_V2: u8` constant is intentionally NOT introduced
// here, to avoid duplicating SSOT.

#[derive(Debug, Clone, Serialize)]
pub struct SummaryUpsertRequest {
    pub schema_version: String,
    pub request_id: String,
    pub summary: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SummaryUpsertResponse {
    pub schema_version: String,
    pub request_id: String,
    pub summary_id: String,
    pub status: String, // "stored" | "stored_with_warnings"
}

/// Structured errors returned by `PhotonClient::upsert_action_summary`.
///
/// DR2-002 / DR1-017: only the strict-mode answer-leak detection produces a
/// structured `Err`. Network / 5xx / timeout failures degrade to `None` at
/// the call site (the existing `send_failopen` pattern).
#[derive(Debug, Clone)]
pub enum PhotonUpsertError {
    /// HTTP 422 with `detail.error="answer_leak_detected"` (photon PR #119
    /// strict-mode rejection). Carries the `quality_warnings` array as
    /// human-readable strings (or `"<non-string>"` for non-string entries).
    AnswerLeakDetected(Vec<String>),
}

impl std::fmt::Display for PhotonUpsertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AnswerLeakDetected(warnings) => write!(
                f,
                "answer leak detected by photon strict-mode: {warnings:?}",
            ),
        }
    }
}

impl std::error::Error for PhotonUpsertError {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_request_serializes_to_three_field_shape() {
        let req = SummaryUpsertRequest {
            schema_version: "action-memory.v0.2".to_string(),
            request_id: "deadbeefcafef00d".to_string(),
            summary: serde_json::json!({
                "summary_id": "anvil-case-x",
                "facts": [],
            }),
        };
        let v = serde_json::to_value(&req).unwrap();
        // top-level keys are exactly the 3 contract fields
        let obj = v.as_object().expect("must be object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["request_id", "schema_version", "summary"]);
        assert_eq!(obj["schema_version"], "action-memory.v0.2");
        assert_eq!(obj["request_id"], "deadbeefcafef00d");
        assert_eq!(obj["summary"]["summary_id"], "anvil-case-x");
    }

    #[test]
    fn upsert_response_deserializes_from_four_field_shape() {
        let body = r#"{
            "schema_version": "action-memory.v0.2",
            "request_id": "deadbeefcafef00d",
            "summary_id": "anvil-case-x",
            "status": "stored"
        }"#;
        let resp: SummaryUpsertResponse = serde_json::from_str(body).unwrap();
        assert_eq!(resp.schema_version, "action-memory.v0.2");
        assert_eq!(resp.request_id, "deadbeefcafef00d");
        assert_eq!(resp.summary_id, "anvil-case-x");
        assert_eq!(resp.status, "stored");
    }

    #[test]
    fn upsert_response_accepts_stored_with_warnings_status() {
        let body = r#"{
            "schema_version": "action-memory.v0.2",
            "request_id": "0011223344556677",
            "summary_id": "anvil-case-y",
            "status": "stored_with_warnings"
        }"#;
        let resp: SummaryUpsertResponse = serde_json::from_str(body).unwrap();
        assert_eq!(resp.status, "stored_with_warnings");
    }

    /// 422 detail.error="answer_leak_detected" body parsing helper.
    /// The actual extraction lives in `client.rs`; here we exercise the JSON
    /// path that the client follows.
    #[test]
    fn answer_leak_detected_body_shape_is_parseable() {
        let body = r#"{
            "detail": {"error": "answer_leak_detected"},
            "quality_warnings": ["facts[0]: literal answer 42", "avoid[1]: numeric"]
        }"#;
        let v: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(v["detail"]["error"], "answer_leak_detected");
        let warnings: Vec<String> = v["quality_warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap_or("<non-string>").to_string())
            .collect();
        let err = PhotonUpsertError::AnswerLeakDetected(warnings.clone());
        assert!(format!("{err}").contains("answer leak"));
        assert_eq!(warnings.len(), 2);
    }
}
