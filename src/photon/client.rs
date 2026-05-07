use std::time::Duration;

use reqwest::blocking::{Client, RequestBuilder};
use serde::de::DeserializeOwned;

use crate::photon::schema::{
    ContextPackRequest, ContextPackResponse, EvaluateRequest, EvaluateResponse, HealthResponse,
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
}
