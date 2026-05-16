//! Parse `EvaluateResponse` into `PhotonEvalSummary` (Issue #558).

use crate::photon::schema::EvaluateResponse;
use crate::session::eval_log::{
    MAX_PHOTON_EVAL_FIELD_BYTES, MAX_PHOTON_EVAL_WARNING_BYTES, MAX_PHOTON_EVAL_WARNINGS,
    PhotonEvalSummary,
};
use crate::session::feedback::mask_secrets;

/// Convert an `EvaluateResponse` into a `PhotonEvalSummary` for the eval log.
///
/// All string fields are masked with `mask_secrets` and byte-truncated before
/// being stored, following the DR4-001 security pipeline.
pub fn parse_evaluate_response(resp: &EvaluateResponse) -> PhotonEvalSummary {
    let v = &resp.0;

    let photon_request_id = extract_str(v, "photon_request_id");
    let context_pack_id = extract_str(v, "context_pack_id");

    // Prefer explicit "admission_decision" string; fall back to "admitted" bool.
    let admission_decision = if let Some(s) = v
        .get("admission_decision")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
    {
        Some(truncate_field(&mask_secrets(s)))
    } else {
        v.get("admitted").and_then(|b| b.as_bool()).map(|admitted| {
            if admitted {
                "accepted".to_string()
            } else {
                "rejected".to_string()
            }
        })
    };

    let prompt_adopted = v.get("admitted").and_then(|b| b.as_bool()).or_else(|| {
        v.get("admission_decision")
            .and_then(|x| x.as_str())
            .map(|s| s == "accepted" || s == "admitted")
    });

    let warnings = v
        .get("warnings")
        .and_then(|w| w.as_array())
        .map(|arr| {
            arr.iter()
                .take(MAX_PHOTON_EVAL_WARNINGS)
                .filter_map(|item| item.as_str())
                .map(|s| truncate_bytes(&mask_secrets(s), MAX_PHOTON_EVAL_WARNING_BYTES))
                .collect()
        })
        .unwrap_or_default();

    let task_outcome = extract_str(v, "task_outcome");
    let retry_summary = extract_str(v, "retry_summary");

    PhotonEvalSummary {
        photon_request_id,
        context_pack_id,
        admission_decision,
        warnings,
        prompt_adopted,
        task_outcome,
        retry_summary,
        // Issue #591 (VR-08 / T2.2): populated by the agent layer
        // (`build_eval_record` site in `turn.rs`) after the cap is applied to
        // the actual evaluate request payload. The parser cannot know the
        // cap-applied count because the request was built in the agent layer.
        summary_ids_adopted_count: None,
        // Issue #601 (DR3-002): photon → session is the only legal direction
        // for value flow. The agent layer populates these in
        // `invoke_photon_evaluate` from the `derive_photon_feedback_outcome`
        // return value; the photon layer here just initializes to `None`.
        outcome_emitted: None,
        outcome_detail_emitted: None,
    }
}

fn extract_str(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| truncate_field(&mask_secrets(s)))
}

fn truncate_field(s: &str) -> String {
    truncate_bytes(s, MAX_PHOTON_EVAL_FIELD_BYTES)
}

fn truncate_bytes(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::photon::schema::EvaluateResponse;

    fn make_resp(json: serde_json::Value) -> EvaluateResponse {
        EvaluateResponse(json)
    }

    #[test]
    fn admitted_true_maps_to_accepted() {
        let resp = make_resp(serde_json::json!({ "admitted": true }));
        let summary = parse_evaluate_response(&resp);
        assert_eq!(summary.admission_decision.as_deref(), Some("accepted"));
        assert_eq!(summary.prompt_adopted, Some(true));
    }

    #[test]
    fn admitted_false_maps_to_rejected() {
        let resp = make_resp(serde_json::json!({ "admitted": false }));
        let summary = parse_evaluate_response(&resp);
        assert_eq!(summary.admission_decision.as_deref(), Some("rejected"));
        assert_eq!(summary.prompt_adopted, Some(false));
    }

    #[test]
    fn explicit_admission_decision_takes_priority() {
        let resp =
            make_resp(serde_json::json!({ "admission_decision": "quarantined", "admitted": true }));
        let summary = parse_evaluate_response(&resp);
        assert_eq!(summary.admission_decision.as_deref(), Some("quarantined"));
    }

    #[test]
    fn warnings_capped_and_masked() {
        let warnings: Vec<serde_json::Value> = (0..20)
            .map(|i| serde_json::json!(format!("warn-{i}")))
            .collect();
        let resp = make_resp(serde_json::json!({ "warnings": warnings }));
        let summary = parse_evaluate_response(&resp);
        assert!(summary.warnings.len() <= MAX_PHOTON_EVAL_WARNINGS);
    }

    #[test]
    fn empty_response_all_none() {
        let resp = make_resp(serde_json::json!({}));
        let summary = parse_evaluate_response(&resp);
        assert!(summary.admission_decision.is_none());
        assert!(summary.context_pack_id.is_none());
        assert!(summary.warnings.is_empty());
    }

    #[test]
    fn context_pack_id_extracted() {
        let resp = make_resp(serde_json::json!({ "context_pack_id": "cpid-xyz" }));
        let summary = parse_evaluate_response(&resp);
        assert_eq!(summary.context_pack_id.as_deref(), Some("cpid-xyz"));
    }

    #[test]
    fn retry_summary_extracted() {
        let resp = make_resp(serde_json::json!({ "retry_summary": "1 retry" }));
        let summary = parse_evaluate_response(&resp);
        assert_eq!(summary.retry_summary.as_deref(), Some("1 retry"));
    }
}
