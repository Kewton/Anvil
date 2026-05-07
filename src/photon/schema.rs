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
