//! HTTP transport abstraction and curl-based implementation.
//!
//! The [`HttpTransport`] trait provides a pluggable HTTP layer.
//! [`CurlHttpTransport`] is the default implementation backed by the
//! `curl` subprocess.

use super::ProviderTurnError;
use std::io::Write;
use std::process::{Command, Stdio};

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
///
/// `post_json_with_headers` and `get_with_headers` are the required methods.
/// `post_json` and `get` have default implementations that delegate to the
/// `_with_headers` variants with an empty header slice.
pub trait HttpTransport {
    /// Send a POST request with JSON body and optional extra headers.
    fn post_json_with_headers(
        &self,
        url: &str,
        body: &[u8],
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse, ProviderTurnError>;

    /// Send a POST request with JSON body (no extra headers).
    fn post_json(&self, url: &str, body: &[u8]) -> Result<HttpResponse, ProviderTurnError> {
        self.post_json_with_headers(url, body, &[])
    }

    /// Send a GET request with optional extra headers.
    fn get_with_headers(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse, ProviderTurnError>;

    /// Send a GET request (no extra headers).
    fn get(&self, url: &str) -> Result<HttpResponse, ProviderTurnError> {
        self.get_with_headers(url, &[])
    }

    /// Stream the response body line-by-line via a callback.
    ///
    /// The default implementation falls back to [`Self::post_json_with_headers`],
    /// splits the body into lines, and calls `on_line` for each.
    /// [`CurlHttpTransport`] overrides this with true streaming using
    /// `curl -N` and unbuffered stdout reading.
    fn stream_lines(
        &self,
        url: &str,
        body: &[u8],
        headers: &[(&str, &str)],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<(), ProviderTurnError> {
        let response = self.post_json_with_headers(url, body, headers)?;
        if response.status_code != 200 {
            let body_text = String::from_utf8_lossy(&response.body);
            return Err(classify_http_error(response.status_code, body_text.trim()));
        }
        for line in String::from_utf8_lossy(&response.body).lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                on_line(trimmed);
            }
        }
        Ok(())
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
    fn post_json_with_headers(
        &self,
        url: &str,
        body: &[u8],
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse, ProviderTurnError> {
        let raw = post_json_with_curl(url, body, headers)?;
        parse_raw_http_response(&raw)
    }

    fn get_with_headers(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse, ProviderTurnError> {
        let raw = get_with_curl(url, headers)?;
        parse_raw_http_response(&raw)
    }

    /// True streaming: spawn curl with `-N` (no buffering) and read lines
    /// from stdout as they arrive.
    fn stream_lines(
        &self,
        url: &str,
        body: &[u8],
        headers: &[(&str, &str)],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<(), ProviderTurnError> {
        curl_stream_lines(url, body, headers, on_line)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Parse a raw HTTP response (as returned by `curl -i`) into status code and body.
pub(crate) fn parse_raw_http_response(raw: &[u8]) -> Result<HttpResponse, ProviderTurnError> {
    let header_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| ProviderTurnError::Parse("invalid HTTP response headers".to_string()))?;
    let headers = &raw[..header_end];
    let body = &raw[header_end + 4..];

    let headers_text = String::from_utf8_lossy(headers);
    let mut header_lines = headers_text.lines();
    let status_line = header_lines
        .next()
        .ok_or_else(|| ProviderTurnError::Parse("missing HTTP status line".to_string()))?;
    let status_code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| ProviderTurnError::Parse("invalid HTTP status line".to_string()))?
        .parse()
        .map_err(|_| ProviderTurnError::Parse("non-numeric HTTP status code".to_string()))?;

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
        let line_end = find_crlf(body, cursor)
            .ok_or_else(|| ProviderTurnError::Parse("invalid chunked response".to_string()))?;
        let size_text = String::from_utf8_lossy(&body[cursor..line_end]);
        let size = usize::from_str_radix(size_text.trim(), 16)
            .map_err(|_| ProviderTurnError::Parse("invalid chunk size in response".to_string()))?;
        cursor = line_end + 2;

        if size == 0 {
            break;
        }

        let chunk_end = cursor
            .checked_add(size)
            .ok_or_else(|| ProviderTurnError::Parse("overflow in chunk size".to_string()))?;
        if chunk_end > body.len() {
            return Err(ProviderTurnError::Parse(
                "truncated chunked response".to_string(),
            ));
        }

        decoded.extend_from_slice(&body[cursor..chunk_end]);
        cursor = chunk_end;

        if body.get(cursor..cursor + 2) != Some(b"\r\n") {
            return Err(ProviderTurnError::Parse(
                "missing chunk terminator in response".to_string(),
            ));
        }
        cursor += 2;
    }

    Ok(decoded)
}

/// Spawn curl in streaming mode (`-N`) and deliver each response line
/// to the callback as it arrives from the server.
/// Default timeout for curl requests in seconds.
const CURL_TIMEOUT_SECS: &str = "300";

fn curl_timeout() -> &'static str {
    // Allow override via environment variable
    static TIMEOUT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TIMEOUT.get_or_init(|| {
        std::env::var("ANVIL_CURL_TIMEOUT").unwrap_or_else(|_| CURL_TIMEOUT_SECS.to_string())
    })
}

fn curl_stream_lines(
    url: &str,
    body: &[u8],
    extra_headers: &[(&str, &str)],
    on_line: &mut dyn FnMut(&str),
) -> Result<(), ProviderTurnError> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "-N", "-X", "POST"])
        .arg("--max-time")
        .arg(curl_timeout())
        .arg("-H")
        .arg("Content-Type: application/json");
    for (name, value) in extra_headers {
        validate_header_value(name, value)?;
        cmd.arg("-H").arg(format!("{name}: {value}"));
    }
    let mut child = cmd
        .arg("--data-binary")
        .arg("@-")
        .arg("--")
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| ProviderTurnError::Network(format!("failed to spawn curl: {err}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(body).map_err(|err| {
            ProviderTurnError::Network(format!("failed to write to curl stdin: {err}"))
        })?;
    }

    if let Some(stdout) = child.stdout.take() {
        let reader = std::io::BufReader::new(stdout);
        use std::io::BufRead;
        for line in reader.lines() {
            let line = line.map_err(|err| {
                ProviderTurnError::Network(format!("failed to read curl output: {err}"))
            })?;
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                on_line(trimmed);
            }
        }
    }

    let status = child
        .wait()
        .map_err(|err| ProviderTurnError::Network(format!("curl process error: {err}")))?;
    if !status.success() {
        let exit_code = status.code().unwrap_or(-1);
        return Err(classify_curl_error(
            exit_code,
            "curl streaming request failed",
        ));
    }

    Ok(())
}

fn find_crlf(body: &[u8], start: usize) -> Option<usize> {
    body[start..]
        .windows(2)
        .position(|window| window == b"\r\n")
        .map(|offset| start + offset)
}

/// Truncate and redact secrets from an error message.
///
/// 1. Truncates to 500 characters (D4-007: prevent information leakage)
/// 2. Redacts Authorization/Bearer/api_key patterns (D4-003: prevent API key leakage)
pub fn sanitize_error_message(message: &str) -> String {
    let truncated = if message.len() > 500 {
        format!(
            "{}... [truncated, {} bytes total]",
            &message[..500],
            message.len()
        )
    } else {
        message.to_string()
    };
    redact_secrets(&truncated)
}

/// Mask known secret patterns in a message string.
///
/// Replaces values following `Authorization:`, `Bearer`, and `api_key:` with `[REDACTED]`.
/// Follows the existing `api_key [REDACTED]` convention from `config/mod.rs`.
pub fn redact_secrets(message: &str) -> String {
    let mut result = String::with_capacity(message.len());
    for line in message.lines() {
        if !result.is_empty() {
            result.push('\n');
        }
        if let Some(pos) = line.to_ascii_lowercase().find("authorization:") {
            let prefix_end = pos + "authorization:".len();
            result.push_str(&line[..prefix_end]);
            result.push_str(" [REDACTED]");
        } else if let Some(pos) = line.to_ascii_lowercase().find("bearer ") {
            let prefix_end = pos + "bearer ".len();
            result.push_str(&line[..prefix_end]);
            result.push_str("[REDACTED]");
        } else if let Some(pos) = line.to_ascii_lowercase().find("api_key:") {
            let prefix_end = pos + "api_key:".len();
            result.push_str(&line[..prefix_end]);
            result.push_str(" [REDACTED]");
        } else {
            result.push_str(line);
        }
    }
    result
}

/// Classify an HTTP error by status code into a typed [`ProviderTurnError`].
///
/// - 400-499 → `ClientError`
/// - 500-599 → `ServerError`
/// - Other   → `Backend` (unclassified)
pub fn classify_http_error(status_code: u16, body: &str) -> ProviderTurnError {
    let sanitized_body = sanitize_error_message(body);
    match status_code {
        400..=499 => ProviderTurnError::ClientError {
            status_code,
            message: sanitized_body,
        },
        500..=599 => ProviderTurnError::ServerError {
            status_code,
            message: sanitized_body,
        },
        _ => {
            ProviderTurnError::Backend(format!("unexpected status {status_code}: {sanitized_body}"))
        }
    }
}

/// Classify a curl exit code into a typed [`ProviderTurnError`].
///
/// - exit 28 → `Timeout`
/// - other   → `Network` (includes DNS failure (6), connection refused (7), etc.)
pub fn classify_curl_error(exit_code: i32, stderr: &str) -> ProviderTurnError {
    let sanitized_stderr = sanitize_error_message(stderr);
    match exit_code {
        28 => ProviderTurnError::Timeout(sanitized_stderr),
        _ => ProviderTurnError::Network(sanitized_stderr),
    }
}

/// Validate that an HTTP header key and value do not contain CRLF characters.
///
/// This prevents HTTP header injection attacks where `\r` or `\n` in a header
/// value could be used to inject additional headers or split the response.
fn validate_header_value(key: &str, value: &str) -> Result<(), ProviderTurnError> {
    if key.contains('\r') || key.contains('\n') || value.contains('\r') || value.contains('\n') {
        return Err(ProviderTurnError::ClientError {
            status_code: 0,
            message: format!(
                "invalid header: header name or value contains newline characters (key: {key})"
            ),
        });
    }
    Ok(())
}

/// Execute a GET request using curl.
///
/// Similar to [`post_json_with_curl`] but without `-X POST`, `--data-binary`,
/// or the `Content-Type` header.  Includes `--proto '=http,https'` for
/// protocol restriction (D4-005) and `--` separator before URL (D4-006).
fn get_with_curl(url: &str, headers: &[(&str, &str)]) -> Result<Vec<u8>, ProviderTurnError> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "--http1.1", "-i"])
        .arg("--proto")
        .arg("=http,https")
        .arg("--max-time")
        .arg(curl_timeout());
    for (name, value) in headers {
        validate_header_value(name, value)?;
        cmd.arg("-H").arg(format!("{name}: {value}"));
    }
    let output = cmd
        .arg("--")
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| ProviderTurnError::Network(format!("failed to spawn curl: {err}")))?
        .wait_with_output()
        .map_err(|err| ProviderTurnError::Network(format!("failed to read curl output: {err}")))?;

    if !output.status.success() {
        let exit_code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(classify_curl_error(exit_code, stderr.trim()));
    }

    Ok(output.stdout)
}

fn post_json_with_curl(
    url: &str,
    body: &[u8],
    extra_headers: &[(&str, &str)],
) -> Result<Vec<u8>, ProviderTurnError> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "--http1.1", "-i", "-X", "POST"])
        .arg("--proto")
        .arg("=http,https")
        .arg("--max-time")
        .arg(curl_timeout())
        .arg("-H")
        .arg("Content-Type: application/json");
    for (name, value) in extra_headers {
        validate_header_value(name, value)?;
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
        .map_err(|err| ProviderTurnError::Network(format!("failed to spawn curl: {err}")))?;

    child
        .stdin
        .as_mut()
        .ok_or_else(|| ProviderTurnError::Network("failed to open curl stdin".to_string()))?
        .write_all(body)
        .map_err(|err| {
            ProviderTurnError::Network(format!("failed to write to curl stdin: {err}"))
        })?;

    let output = child
        .wait_with_output()
        .map_err(|err| ProviderTurnError::Network(format!("failed to read curl output: {err}")))?;

    if !output.status.success() {
        let exit_code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(classify_curl_error(exit_code, stderr.trim()));
    }

    Ok(output.stdout)
}

// ---------------------------------------------------------------------------
// RetryTransport
// ---------------------------------------------------------------------------

/// Configuration for the retry mechanism.
pub struct RetryConfig {
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub backoff_factor: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 1000,
            backoff_factor: 2,
            max_delay_ms: 30_000,
        }
    }
}

/// Transport wrapper that retries retryable errors with exponential backoff.
///
/// Wraps any [`HttpTransport`] implementation and automatically retries
/// operations that fail with retryable errors (network, server, timeout).
pub struct RetryTransport<T: HttpTransport> {
    inner: T,
    config: RetryConfig,
}

impl<T: HttpTransport> RetryTransport<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            config: RetryConfig::default(),
        }
    }

    pub fn with_config(inner: T, config: RetryConfig) -> Self {
        Self { inner, config }
    }

    /// Common retry loop with an additional guard closure.
    ///
    /// The `guard` closure is checked after each failed attempt. If it returns
    /// `false`, the retry loop stops even if the error is retryable. This is
    /// used by `stream_lines` to prevent retrying after the callback has been
    /// invoked (since callback side-effects cannot be rolled back).
    fn retry_with_guard<F, G, R>(&self, mut operation: F, guard: G) -> Result<R, ProviderTurnError>
    where
        F: FnMut() -> Result<R, ProviderTurnError>,
        G: Fn() -> bool,
    {
        let mut last_error = None;
        for attempt in 0..=self.config.max_retries {
            match operation() {
                Ok(result) => return Ok(result),
                Err(err) if err.is_retryable() && guard() && attempt < self.config.max_retries => {
                    let delay = self
                        .config
                        .base_delay_ms
                        .saturating_mul(self.config.backoff_factor.saturating_pow(attempt))
                        .min(self.config.max_delay_ms);
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                    last_error = Some(err);
                }
                Err(err) => return Err(err),
            }
        }
        Err(last_error.unwrap())
    }
}

impl<T: HttpTransport> HttpTransport for RetryTransport<T> {
    fn post_json_with_headers(
        &self,
        url: &str,
        body: &[u8],
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse, ProviderTurnError> {
        self.retry_with_guard(
            || self.inner.post_json_with_headers(url, body, headers),
            || true,
        )
    }

    fn get_with_headers(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse, ProviderTurnError> {
        self.retry_with_guard(|| self.inner.get_with_headers(url, headers), || true)
    }

    fn stream_lines(
        &self,
        url: &str,
        body: &[u8],
        headers: &[(&str, &str)],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<(), ProviderTurnError> {
        let callback_invoked = std::cell::Cell::new(false);
        let mut wrapped_on_line = |line: &str| {
            callback_invoked.set(true);
            on_line(line);
        };

        self.retry_with_guard(
            || {
                callback_invoked.set(false);
                self.inner
                    .stream_lines(url, body, headers, &mut wrapped_on_line)
            },
            || !callback_invoked.get(),
        )
    }
}
