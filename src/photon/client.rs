use std::time::Duration;

use reqwest::blocking::{Client, RequestBuilder};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::photon::prompt::sanitize_summary_id;
use crate::photon::schema::{
    ContextPackRequest, ContextPackResponse, EvaluateRequest, EvaluateResponse, HealthResponse,
    PhotonUpsertError, SummaryUpsertRequest, SummaryUpsertResponse,
};
use crate::session::feedback::mask_secrets;

pub struct PhotonClient {
    base_url: String,
    http: Client,
}

impl PhotonClient {
    pub fn new(base_url: String, timeout_ms: u64) -> Result<Self, String> {
        let timeout = Duration::from_millis(timeout_ms);
        let http = Client::builder()
            .connect_timeout(timeout)
            .timeout(timeout)
            .build()
            .map_err(|e| format!("failed to build photon http client: {e}"))?;
        Ok(Self { base_url, http })
    }

    fn send_failopen<T: DeserializeOwned>(&self, req: RequestBuilder, context: &str) -> Option<T> {
        match req
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.json::<T>())
        {
            Ok(value) => Some(value),
            Err(err) => {
                let masked = mask_secrets(&err.to_string());
                tracing::warn!("photon {context} failed (fail-open): {masked}");
                None
            }
        }
    }

    pub fn health(&self) -> bool {
        let req = self.http.get(format!("{}/health", self.base_url));
        self.send_failopen::<HealthResponse>(req, "GET /health")
            .map(|r| r.ok)
            .unwrap_or(false)
    }

    pub fn context_pack(&self, req: &ContextPackRequest) -> Option<ContextPackResponse> {
        let request = self
            .http
            .post(format!("{}/v1/context/pack", self.base_url))
            .json(req);
        self.send_failopen(request, "POST /v1/context/pack")
    }

    pub fn evaluate(&self, req: &EvaluateRequest) -> Option<EvaluateResponse> {
        let request = self
            .http
            .post(format!("{}/v1/evaluate", self.base_url))
            .json(req);
        self.send_failopen(request, "POST /v1/evaluate")
    }

    /// POST `/v1/summary/upsert` (Issue #604, photon-action-memory PR #119/#120).
    ///
    /// Synchronous API following the existing `send_failopen` pattern: network
    /// / 5xx / timeout failures degrade to `None` (fail-open), while HTTP 200
    /// returns `Some(Ok(_))` and HTTP 422 with the strict-mode
    /// `answer_leak_detected` body returns `Some(Err(AnswerLeakDetected))`.
    ///
    /// DR1-010 / DR2-003 two-stage defense: re-runs `sanitize_summary_id` on
    /// `summary["summary_id"]` immediately before serializing the POST body
    /// (the first stage runs in the agent layer hook). If sanitization yields
    /// `None`, the call short-circuits to `None` (fail-open) and emits a
    /// `tracing::warn!`.
    pub fn upsert_action_summary(
        &self,
        schema_version: &str,
        request_id: &str,
        mut summary: Value,
    ) -> Option<Result<SummaryUpsertResponse, PhotonUpsertError>> {
        // Stage-2 defense: re-sanitize summary_id.
        let raw_id = summary
            .get("summary_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let clean_id = match raw_id.as_deref().and_then(sanitize_summary_id) {
            Some(id) => id,
            None => {
                tracing::warn!(
                    "photon POST /v1/summary/upsert skipped (fail-open): \
                     summary_id failed sanitize_summary_id (stage-2 defense)",
                );
                return None;
            }
        };
        if let Some(slot) = summary.get_mut("summary_id") {
            *slot = Value::String(clean_id);
        }

        let body = SummaryUpsertRequest {
            schema_version: schema_version.to_string(),
            request_id: request_id.to_string(),
            summary,
        };
        let request = self
            .http
            .post(format!("{}/v1/summary/upsert", self.base_url))
            .json(&body);

        let response = match request.send() {
            Ok(r) => r,
            Err(err) => {
                let masked = mask_secrets(&err.to_string());
                tracing::warn!("photon POST /v1/summary/upsert failed (fail-open): {masked}");
                return None;
            }
        };

        let status = response.status();

        if status.is_success() {
            return match response.json::<SummaryUpsertResponse>() {
                Ok(parsed) => Some(Ok(parsed)),
                Err(err) => {
                    let masked = mask_secrets(&err.to_string());
                    tracing::warn!(
                        "photon POST /v1/summary/upsert 200 deserialize failed \
                         (fail-open): {masked}",
                    );
                    None
                }
            };
        }

        if status.as_u16() == 422 {
            // Strict-mode answer_leak_detected returns a structured error.
            // Other 422 bodies (e.g. validation failure) also degrade to
            // fail-open None to keep the agent loop alive.
            let text = response.text().unwrap_or_default();
            let parsed: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => {
                    let masked = mask_secrets(&text);
                    tracing::warn!(
                        "photon POST /v1/summary/upsert 422 non-JSON body \
                         (fail-open): {masked}",
                    );
                    return None;
                }
            };
            let leak_marker = parsed
                .get("detail")
                .and_then(|d| d.get("error"))
                .and_then(Value::as_str)
                == Some("answer_leak_detected");
            if leak_marker {
                let warnings: Vec<String> = parsed
                    .get("quality_warnings")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .map(|w| {
                                w.as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| w.to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                return Some(Err(PhotonUpsertError::AnswerLeakDetected(warnings)));
            }
            tracing::warn!("photon POST /v1/summary/upsert 422 (non-answer-leak, fail-open)",);
            return None;
        }

        // 4xx (other), 5xx, redirect, etc. → fail-open None.
        let snippet = mask_secrets(&response.text().unwrap_or_default());
        tracing::warn!("photon POST /v1/summary/upsert returned {status} (fail-open): {snippet}",);
        None
    }
}
