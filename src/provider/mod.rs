/// Provider integration layer.
///
/// Abstracts LLM provider communication behind the [`ProviderClient`] trait,
/// allowing both Ollama and OpenAI-compatible backends to share the same
/// application-level flow.  HTTP transport is pluggable via [`HttpTransport`].
pub mod openai;

use crate::agent::AgentEvent;
use crate::config::EffectiveConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

/// Supported LLM provider backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBackend {
    Ollama,
    OpenAi,
}

/// Dispatch enum for concrete provider clients.
pub enum LocalProviderClient {
    Ollama(OllamaProviderClient),
    OpenAi(openai::OpenAiCompatibleProviderClient),
}

/// Feature flags discovered (or assumed) for a provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub streaming: bool,
    pub tool_calling: bool,
}

/// Bootstrapped provider context available for the lifetime of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderRuntimeContext {
    pub backend: ProviderBackend,
    pub capabilities: ProviderCapabilities,
}

/// Message role used in provider requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderMessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// A single message in a provider request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderMessage {
    pub role: ProviderMessageRole,
    pub content: String,
}

impl ProviderMessage {
    pub fn new(role: ProviderMessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }
}

/// Request payload sent to a provider for one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTurnRequest {
    pub model: String,
    pub messages: Vec<ProviderMessage>,
    pub stream: bool,
}

impl ProviderTurnRequest {
    pub fn new(model: String, messages: Vec<ProviderMessage>, stream: bool) -> Self {
        Self {
            model,
            messages,
            stream,
        }
    }
}

/// Events emitted by a provider during a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderEvent {
    Agent(AgentEvent),
    TokenDelta(String),
}

/// Errors that can occur during a provider turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderTurnError {
    Cancelled,
    Backend(String),
}

/// Classification of provider errors for persistence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderErrorKind {
    Cancelled,
    Backend,
}

/// A provider error record stored in the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderErrorRecord {
    pub kind: ProviderErrorKind,
    pub message: String,
}

impl std::fmt::Display for ProviderTurnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "provider turn cancelled"),
            Self::Backend(message) => write!(f, "provider backend error: {message}"),
        }
    }
}

impl std::error::Error for ProviderTurnError {}

/// Abstraction over LLM provider communication.
///
/// Implementors receive a request and emit [`ProviderEvent`]s via the
/// provided callback.
pub trait ProviderClient {
    fn stream_turn(
        &self,
        request: &ProviderTurnRequest,
        emit: &mut dyn FnMut(ProviderEvent),
    ) -> Result<(), ProviderTurnError>;
}

// ---------------------------------------------------------------------------
// HTTP transport abstraction
// ---------------------------------------------------------------------------

/// Parsed HTTP response returned by an [`HttpTransport`] implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status_code: u16,
    pub body: Vec<u8>,
}

/// Low-level HTTP transport used by provider clients.
///
/// The trait is intentionally simple so that it can be backed by `curl`,
/// a Rust HTTP library, or a test mock.
pub trait HttpTransport {
    fn post_json(&self, url: &str, body: &[u8]) -> Result<HttpResponse, ProviderTurnError>;

    fn post_json_with_headers(
        &self,
        url: &str,
        body: &[u8],
        _headers: &[(&str, &str)],
    ) -> Result<HttpResponse, ProviderTurnError> {
        self.post_json(url, body)
    }
}

/// HTTP transport backed by the `curl` subprocess.
///
/// This is the default transport.  It works on any system where `curl` is
/// installed and avoids pulling in native TLS dependencies.
pub struct CurlHttpTransport;

/// Backward-compatible alias.
pub type TcpHttpTransport = CurlHttpTransport;

impl HttpTransport for CurlHttpTransport {
    fn post_json(&self, url: &str, body: &[u8]) -> Result<HttpResponse, ProviderTurnError> {
        let raw = post_json_with_curl(url, body, &[])?;
        parse_raw_http_response(&raw)
    }

    fn post_json_with_headers(
        &self,
        url: &str,
        body: &[u8],
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse, ProviderTurnError> {
        let raw = post_json_with_curl(url, body, headers)?;
        parse_raw_http_response(&raw)
    }
}

// ---------------------------------------------------------------------------
// Ollama provider
// ---------------------------------------------------------------------------

/// Wire format for Ollama chat messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaChatMessage {
    pub role: String,
    pub content: String,
}

/// Wire format for an Ollama `/api/chat` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaChatRequest {
    pub model: String,
    pub messages: Vec<OllamaChatMessage>,
    pub stream: bool,
    #[serde(default)]
    pub think: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OllamaChatChunk {
    #[serde(default)]
    message: Option<OllamaChatMessage>,
    #[serde(default)]
    done: bool,
}

/// Client for the Ollama local inference server.
///
/// Generic over [`HttpTransport`] so tests can inject a mock.
pub struct OllamaProviderClient<T = CurlHttpTransport> {
    base_url: String,
    transport: T,
}

impl OllamaProviderClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            transport: CurlHttpTransport,
        }
    }

    pub fn from_config(config: &EffectiveConfig) -> Self {
        Self::new(config.runtime.provider_url.clone())
    }
}

impl<T> OllamaProviderClient<T> {
    pub fn with_transport(base_url: impl Into<String>, transport: T) -> Self {
        Self {
            base_url: base_url.into(),
            transport,
        }
    }

    pub fn build_chat_request(request: &ProviderTurnRequest) -> OllamaChatRequest {
        OllamaChatRequest {
            model: request.model.clone(),
            messages: request
                .messages
                .iter()
                .map(|message| OllamaChatMessage {
                    role: match message.role {
                        ProviderMessageRole::System => "system".to_string(),
                        ProviderMessageRole::User => "user".to_string(),
                        ProviderMessageRole::Assistant => "assistant".to_string(),
                        ProviderMessageRole::Tool => "tool".to_string(),
                    },
                    content: message.content.clone(),
                })
                .collect(),
            stream: request.stream,
            think: false,
        }
    }

    pub fn normalize_stream_chunks(
        chunks: &[String],
    ) -> Result<Vec<ProviderEvent>, ProviderTurnError> {
        let mut events = Vec::new();
        let mut assistant_output = String::new();

        for chunk in chunks {
            let parsed: OllamaChatChunk = serde_json::from_str(chunk).map_err(|err| {
                ProviderTurnError::Backend(format!("invalid ollama response: {err}"))
            })?;

            if let Some(message) = parsed.message
                && !message.content.is_empty()
            {
                assistant_output.push_str(&message.content);
                events.push(ProviderEvent::TokenDelta(message.content));
            }

            if parsed.done {
                events.push(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: assistant_output.clone(),
                    completion_summary: "Provider turn finished successfully.".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                }));
            }
        }

        Ok(events)
    }
}

pub fn resolve_ollama_model_alias(requested: &str, available: &[String]) -> String {
    if available.iter().any(|name| name == requested) {
        return requested.to_string();
    }

    let mut prefix_matches = available
        .iter()
        .filter(|name| name.starts_with(requested))
        .cloned()
        .collect::<Vec<_>>();
    prefix_matches.sort();
    prefix_matches.dedup();

    if prefix_matches.len() == 1 {
        prefix_matches.remove(0)
    } else {
        requested.to_string()
    }
}

impl<T: HttpTransport> OllamaProviderClient<T> {
    fn send_chat_request(
        &self,
        request: &ProviderTurnRequest,
    ) -> Result<Vec<String>, ProviderTurnError> {
        let resolved_model = resolve_model_with_ollama_tags(&self.base_url, &request.model);
        let mut resolved_request = request.clone();
        resolved_request.model = resolved_model;
        let chat_request = Self::build_chat_request(&resolved_request);
        let request_body = serde_json::to_vec(&chat_request).map_err(|err| {
            ProviderTurnError::Backend(format!("failed to encode ollama request: {err}"))
        })?;
        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));

        let response = self.transport.post_json(&url, &request_body)?;
        if response.status_code != 200 {
            let body_text = String::from_utf8_lossy(&response.body);
            return Err(ProviderTurnError::Backend(format!(
                "ollama request failed with status {}: {}",
                response.status_code,
                body_text.trim()
            )));
        }

        Ok(parse_ndjson_lines(&response.body))
    }
}

impl Default for OllamaProviderClient {
    fn default() -> Self {
        Self::new("http://127.0.0.1:11434")
    }
}

impl<T: HttpTransport> ProviderClient for OllamaProviderClient<T> {
    fn stream_turn(
        &self,
        request: &ProviderTurnRequest,
        emit: &mut dyn FnMut(ProviderEvent),
    ) -> Result<(), ProviderTurnError> {
        let chunks = self.send_chat_request(request)?;
        for event in Self::normalize_stream_chunks(&chunks)? {
            emit(event);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Bootstrap / dispatch
// ---------------------------------------------------------------------------

/// Errors during provider bootstrap.
#[derive(Debug)]
pub enum ProviderBootstrapError {
    UnsupportedBackend(String),
}

impl std::fmt::Display for ProviderBootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedBackend(backend) => {
                write!(f, "unsupported provider backend: {backend}")
            }
        }
    }
}

impl std::error::Error for ProviderBootstrapError {}

impl ProviderRuntimeContext {
    pub fn bootstrap(config: &EffectiveConfig) -> Result<Self, ProviderBootstrapError> {
        let backend = match config.runtime.provider.as_str() {
            "ollama" => ProviderBackend::Ollama,
            "openai" => ProviderBackend::OpenAi,
            other => {
                return Err(ProviderBootstrapError::UnsupportedBackend(
                    other.to_string(),
                ));
            }
        };

        let capabilities = match backend {
            ProviderBackend::Ollama => ProviderCapabilities {
                streaming: true,
                tool_calling: true,
            },
            ProviderBackend::OpenAi => ProviderCapabilities {
                streaming: true,
                tool_calling: true,
            },
        };

        Ok(Self {
            backend,
            capabilities,
        })
    }
}

impl ProviderClient for LocalProviderClient {
    fn stream_turn(
        &self,
        request: &ProviderTurnRequest,
        emit: &mut dyn FnMut(ProviderEvent),
    ) -> Result<(), ProviderTurnError> {
        match self {
            Self::Ollama(client) => client.stream_turn(request, emit),
            Self::OpenAi(client) => client.stream_turn(request, emit),
        }
    }
}

/// Build a concrete provider client from the effective config.
pub fn build_local_provider_client(
    config: &EffectiveConfig,
) -> Result<LocalProviderClient, ProviderBootstrapError> {
    match config.runtime.provider.as_str() {
        "ollama" => Ok(LocalProviderClient::Ollama(
            OllamaProviderClient::from_config(config),
        )),
        "openai" => Ok(LocalProviderClient::OpenAi(
            openai::OpenAiCompatibleProviderClient::from_config(config),
        )),
        other => Err(ProviderBootstrapError::UnsupportedBackend(
            other.to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Parse NDJSON (newline-delimited JSON) body into individual lines.
fn parse_ndjson_lines(body: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(body)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Parse a raw HTTP response (as returned by `curl -i`) into status code and body.
fn parse_raw_http_response(raw: &[u8]) -> Result<HttpResponse, ProviderTurnError> {
    let header_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| ProviderTurnError::Backend("invalid HTTP response headers".to_string()))?;
    let headers = &raw[..header_end];
    let body = &raw[header_end + 4..];

    let headers_text = String::from_utf8_lossy(headers);
    let mut header_lines = headers_text.lines();
    let status_line = header_lines
        .next()
        .ok_or_else(|| ProviderTurnError::Backend("missing HTTP status line".to_string()))?;
    let status_code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| ProviderTurnError::Backend("invalid HTTP status line".to_string()))?
        .parse()
        .map_err(|_| ProviderTurnError::Backend("non-numeric HTTP status code".to_string()))?;

    let is_chunked = header_lines.any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("transfer-encoding:") && lower.contains("chunked")
    });

    let decoded_body = if is_chunked {
        match decode_chunked_body(body) {
            Ok(decoded) => decoded,
            Err(_) => body.to_vec(),
        }
    } else {
        body.to_vec()
    };

    Ok(HttpResponse {
        status_code,
        body: decoded_body,
    })
}

fn decode_chunked_body(body: &[u8]) -> Result<Vec<u8>, ProviderTurnError> {
    let mut decoded = Vec::new();
    let mut cursor = 0usize;

    while cursor < body.len() {
        let line_end = find_crlf(body, cursor).ok_or_else(|| {
            ProviderTurnError::Backend("invalid chunked response".to_string())
        })?;
        let size_text = String::from_utf8_lossy(&body[cursor..line_end]);
        let size = usize::from_str_radix(size_text.trim(), 16).map_err(|_| {
            ProviderTurnError::Backend("invalid chunk size in response".to_string())
        })?;
        cursor = line_end + 2;

        if size == 0 {
            break;
        }

        let chunk_end = cursor.checked_add(size).ok_or_else(|| {
            ProviderTurnError::Backend("overflow in chunk size".to_string())
        })?;
        if chunk_end > body.len() {
            return Err(ProviderTurnError::Backend(
                "truncated chunked response".to_string(),
            ));
        }

        decoded.extend_from_slice(&body[cursor..chunk_end]);
        cursor = chunk_end;

        if body.get(cursor..cursor + 2) != Some(b"\r\n") {
            return Err(ProviderTurnError::Backend(
                "missing chunk terminator in response".to_string(),
            ));
        }
        cursor += 2;
    }

    Ok(decoded)
}

fn resolve_model_with_ollama_tags(base_url: &str, requested: &str) -> String {
    let url = format!("{}/api/tags", base_url.trim_end_matches('/'));
    let output = Command::new("curl")
        .arg("-sS")
        .arg("--max-time")
        .arg("5")
        .arg(url)
        .output();

    let Ok(output) = output else {
        return requested.to_string();
    };
    if !output.status.success() {
        return requested.to_string();
    }

    let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) else {
        return requested.to_string();
    };
    let Some(models) = value.get("models").and_then(Value::as_array) else {
        return requested.to_string();
    };
    let names = models
        .iter()
        .filter_map(|model| model.get("name").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    resolve_ollama_model_alias(requested, &names)
}

fn find_crlf(body: &[u8], start: usize) -> Option<usize> {
    body[start..]
        .windows(2)
        .position(|window| window == b"\r\n")
        .map(|offset| start + offset)
}

fn post_json_with_curl(
    url: &str,
    body: &[u8],
    extra_headers: &[(&str, &str)],
) -> Result<Vec<u8>, ProviderTurnError> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "--http1.1", "-i", "-X", "POST"])
        .arg("-H")
        .arg("Content-Type: application/json");
    for (name, value) in extra_headers {
        cmd.arg("-H").arg(format!("{name}: {value}"));
    }
    let mut child = cmd
        .arg("--data-binary")
        .arg("@-")
        .arg("--")
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            ProviderTurnError::Backend(format!("failed to spawn curl: {err}"))
        })?;

    child
        .stdin
        .as_mut()
        .ok_or_else(|| {
            ProviderTurnError::Backend("failed to open curl stdin".to_string())
        })?
        .write_all(body)
        .map_err(|err| {
            ProviderTurnError::Backend(format!("failed to write to curl stdin: {err}"))
        })?;

    let output = child.wait_with_output().map_err(|err| {
        ProviderTurnError::Backend(format!("failed to read curl output: {err}"))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProviderTurnError::Backend(format!(
            "curl request failed: {}",
            stderr.trim()
        )));
    }

    Ok(output.stdout)
}
