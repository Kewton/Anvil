use crate::agent::AgentEvent;
use crate::config::EffectiveConfig;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBackend {
    Ollama,
}

pub enum LocalProviderClient {
    Ollama(OllamaProviderClient),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub streaming: bool,
    pub tool_calling: bool,
}

impl Default for ProviderCapabilities {
    fn default() -> Self {
        Self {
            streaming: false,
            tool_calling: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderRuntimeContext {
    pub backend: ProviderBackend,
    pub capabilities: ProviderCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderMessageRole {
    System,
    User,
    Assistant,
    Tool,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderEvent {
    Agent(AgentEvent),
    TokenDelta(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderTurnError {
    Cancelled,
    Backend(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderErrorKind {
    Cancelled,
    Backend,
}

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

pub trait ProviderClient {
    fn stream_turn(
        &self,
        request: &ProviderTurnRequest,
        emit: &mut dyn FnMut(ProviderEvent),
    ) -> Result<(), ProviderTurnError>;
}

pub trait HttpTransport {
    fn post_json(
        &self,
        authority: &str,
        host: &str,
        port: u16,
        path: &str,
        body: &[u8],
    ) -> Result<Vec<u8>, ProviderTurnError>;
}

pub struct TcpHttpTransport;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaChatMessage {
    pub role: String,
    pub content: String,
}

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

pub struct OllamaProviderClient<T = TcpHttpTransport> {
    base_url: String,
    transport: T,
}

impl OllamaProviderClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            transport: TcpHttpTransport,
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

            if let Some(message) = parsed.message {
                if !message.content.is_empty() {
                    assistant_output.push_str(&message.content);
                    events.push(ProviderEvent::TokenDelta(message.content));
                }
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

impl<T: HttpTransport> OllamaProviderClient<T> {
    fn send_chat_request(
        &self,
        request: &ProviderTurnRequest,
    ) -> Result<Vec<String>, ProviderTurnError> {
        let endpoint = parse_http_base_url(&self.base_url)?;
        let chat_request = Self::build_chat_request(request);
        let request_body = serde_json::to_vec(&chat_request).map_err(|err| {
            ProviderTurnError::Backend(format!("failed to encode ollama request: {err}"))
        })?;
        let request_path = format!("{}/api/chat", endpoint.path_prefix);

        let response = self.transport.post_json(
            &endpoint.authority,
            &endpoint.host,
            endpoint.port,
            &request_path,
            &request_body,
        )?;
        decode_ollama_response(&response)
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

impl HttpTransport for TcpHttpTransport {
    fn post_json(
        &self,
        authority: &str,
        host: &str,
        port: u16,
        path: &str,
        body: &[u8],
    ) -> Result<Vec<u8>, ProviderTurnError> {
        let url = format!("http://{host}:{port}{path}");
        post_json_with_curl(&url, authority, host, body)
    }
}

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
        // Phase 2 bootstrap uses backend-level preset capabilities.
        // Later phases can replace or refine this with live capability discovery.
        let backend = match config.runtime.provider.as_str() {
            "ollama" => ProviderBackend::Ollama,
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
        }
    }
}

pub fn build_local_provider_client(
    config: &EffectiveConfig,
) -> Result<LocalProviderClient, ProviderBootstrapError> {
    match config.runtime.provider.as_str() {
        "ollama" => Ok(LocalProviderClient::Ollama(
            OllamaProviderClient::from_config(config),
        )),
        other => Err(ProviderBootstrapError::UnsupportedBackend(
            other.to_string(),
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedHttpEndpoint {
    authority: String,
    host: String,
    port: u16,
    path_prefix: String,
}

fn parse_http_base_url(base_url: &str) -> Result<ParsedHttpEndpoint, ProviderTurnError> {
    let without_scheme = base_url.strip_prefix("http://").ok_or_else(|| {
        ProviderTurnError::Backend("ollama base url must use http://".to_string())
    })?;
    let (authority, path) = match without_scheme.split_once('/') {
        Some((authority, path)) => (authority, format!("/{}", path.trim_matches('/'))),
        None => (without_scheme, String::new()),
    };

    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => {
            let port = port.parse::<u16>().map_err(|_| {
                ProviderTurnError::Backend(format!("invalid ollama port in base url: {base_url}"))
            })?;
            (host.to_string(), port)
        }
        None => (authority.to_string(), 80),
    };

    Ok(ParsedHttpEndpoint {
        authority: authority.to_string(),
        host,
        port,
        path_prefix: path,
    })
}

fn decode_ollama_response(response: &[u8]) -> Result<Vec<String>, ProviderTurnError> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| ProviderTurnError::Backend("invalid ollama response headers".to_string()))?;
    let headers = &response[..header_end];
    let body = &response[header_end + 4..];
    let headers_text = String::from_utf8_lossy(headers);
    let mut header_lines = headers_text.lines();
    let status_line = header_lines
        .next()
        .ok_or_else(|| ProviderTurnError::Backend("missing ollama status line".to_string()))?;
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| ProviderTurnError::Backend("invalid ollama status line".to_string()))?;
    if status_code != "200" {
        let response_body = String::from_utf8_lossy(body);
        return Err(ProviderTurnError::Backend(format!(
            "ollama request failed with status {status_code}: {}",
            response_body.trim()
        )));
    }

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

    Ok(String::from_utf8_lossy(&decoded_body)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn decode_chunked_body(body: &[u8]) -> Result<Vec<u8>, ProviderTurnError> {
    let mut decoded = Vec::new();
    let mut cursor = 0usize;

    while cursor < body.len() {
        let line_end = find_crlf(body, cursor).ok_or_else(|| {
            ProviderTurnError::Backend("invalid chunked ollama response".to_string())
        })?;
        let size_text = String::from_utf8_lossy(&body[cursor..line_end]);
        let size = usize::from_str_radix(size_text.trim(), 16).map_err(|_| {
            ProviderTurnError::Backend("invalid chunk size in ollama response".to_string())
        })?;
        cursor = line_end + 2;

        if size == 0 {
            break;
        }

        let chunk_end = cursor.checked_add(size).ok_or_else(|| {
            ProviderTurnError::Backend("overflow in ollama chunk size".to_string())
        })?;
        if chunk_end > body.len() {
            return Err(ProviderTurnError::Backend(
                "truncated ollama chunked response".to_string(),
            ));
        }

        decoded.extend_from_slice(&body[cursor..chunk_end]);
        cursor = chunk_end;

        if body.get(cursor..cursor + 2) != Some(b"\r\n") {
            return Err(ProviderTurnError::Backend(
                "missing chunk terminator in ollama response".to_string(),
            ));
        }
        cursor += 2;
    }

    Ok(decoded)
}

fn find_crlf(body: &[u8], start: usize) -> Option<usize> {
    body[start..]
        .windows(2)
        .position(|window| window == b"\r\n")
        .map(|offset| start + offset)
}

fn post_json_with_curl(
    url: &str,
    authority: &str,
    host: &str,
    body: &[u8],
) -> Result<Vec<u8>, ProviderTurnError> {
    let mut child = Command::new("curl")
        .args([
            "-sS",
            "--http1.1",
            "-i",
            "-X",
            "POST",
            "-H",
            &format!("Host: {authority}"),
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
            url,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            ProviderTurnError::Backend(format!("failed to connect to ollama via curl: {err}"))
        })?;

    child
        .stdin
        .as_mut()
        .ok_or_else(|| {
            ProviderTurnError::Backend("failed to open curl stdin for ollama request".to_string())
        })?
        .write_all(body)
        .map_err(|err| {
            ProviderTurnError::Backend(format!("failed to send ollama request via curl: {err}"))
        })?;

    let output = child.wait_with_output().map_err(|err| {
        ProviderTurnError::Backend(format!("failed to read ollama response via curl: {err}"))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ProviderTurnError::Backend(format!(
            "failed to connect to ollama via curl for host {host}: {}",
            stderr.trim()
        )));
    }

    Ok(output.stdout)
}
