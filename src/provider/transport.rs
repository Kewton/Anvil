//! HTTP transport abstraction and reqwest-based implementation.
//!
//! The [`HttpTransport`] trait provides a pluggable HTTP layer.
//! [`ReqwestHttpTransport`] is the default implementation backed by
//! `reqwest::blocking::Client`.

use super::ProviderTurnError;
use std::io::Read as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Parsed HTTP response returned by an [`HttpTransport`] implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status_code: u16,
    pub body: Vec<u8>,
}

/// Low-level HTTP transport used by provider clients.
///
/// The trait is intentionally simple so that it can be backed by any
/// HTTP library or a test mock.
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
    /// [`ReqwestHttpTransport`] overrides this with true streaming using
    /// `BufReader::lines()` on the response body.
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

/// Maximum response body size for provider requests (50 MB).
const MAX_PROVIDER_RESPONSE_SIZE: u64 = 50 * 1024 * 1024;

/// Validate that a URL uses only http or https scheme.
fn validate_url_scheme(url: &str) -> Result<(), ProviderTurnError> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ProviderTurnError::Network(format!(
            "Unsupported URL scheme: only http/https allowed, got: {}",
            url
        )));
    }
    Ok(())
}

/// Map a `reqwest::Error` to a [`ProviderTurnError`].
pub fn classify_reqwest_error(err: reqwest::Error) -> ProviderTurnError {
    if err.is_timeout() {
        ProviderTurnError::Timeout(sanitize_error_message(&err.to_string()))
    } else if err.is_connect() {
        let msg = err.to_string();
        if msg.contains("dns")
            || msg.contains("resolve")
            || msg.contains("Name or service not known")
        {
            ProviderTurnError::DnsFailure(sanitize_error_message(&msg))
        } else {
            ProviderTurnError::ConnectionRefused(sanitize_error_message(&msg))
        }
    } else {
        ProviderTurnError::Network(sanitize_error_message(&err.to_string()))
    }
}

/// Default HTTP request timeout in seconds.
pub const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 300;

/// Normalize timeout values shared by config validation and env fallback.
///
/// - 0 → `DEFAULT_HTTP_TIMEOUT_SECS` (restore default)
/// - <10 → 10
/// - >3600 → 3600
/// - otherwise → unchanged
pub fn normalize_http_timeout(timeout_secs: u64) -> u64 {
    const MIN_HTTP_TIMEOUT_SECS: u64 = 10;
    const MAX_HTTP_TIMEOUT_SECS: u64 = 3600;

    if timeout_secs == 0 {
        DEFAULT_HTTP_TIMEOUT_SECS
    } else {
        timeout_secs.clamp(MIN_HTTP_TIMEOUT_SECS, MAX_HTTP_TIMEOUT_SECS)
    }
}

/// Read the HTTP timeout from the environment.
///
/// Checks `ANVIL_HTTP_TIMEOUT` first, then falls back to `ANVIL_CURL_TIMEOUT`
/// for backward compatibility, defaulting to `DEFAULT_HTTP_TIMEOUT_SECS`.
pub fn http_timeout() -> u64 {
    let parsed = std::env::var("ANVIL_HTTP_TIMEOUT")
        .or_else(|_| std::env::var("ANVIL_CURL_TIMEOUT"))
        .unwrap_or_else(|_| DEFAULT_HTTP_TIMEOUT_SECS.to_string())
        .parse::<u64>()
        .unwrap_or(DEFAULT_HTTP_TIMEOUT_SECS);
    normalize_http_timeout(parsed)
}

/// HTTP transport backed by `reqwest::blocking::Client`.
pub struct ReqwestHttpTransport {
    /// Non-streaming client with full request timeout.
    client: reqwest::blocking::Client,
    /// Streaming client with connect-timeout only (no request timeout).
    streaming_client: reqwest::blocking::Client,
    /// Cancellation flag.
    shutdown_flag: Option<Arc<AtomicBool>>,
}

impl Default for ReqwestHttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestHttpTransport {
    /// Create a new transport with explicit timeout.
    pub fn with_timeout(timeout_secs: u64) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Failed to build HTTP client");
        let streaming_client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Failed to build streaming HTTP client");
        Self {
            client,
            streaming_client,
            shutdown_flag: None,
        }
    }

    /// Create a new transport (reads timeout from environment).
    pub fn new() -> Self {
        Self::with_timeout(http_timeout())
    }

    /// Create a new transport with explicit timeout and shutdown flag.
    pub fn with_timeout_and_shutdown_flag(
        timeout_secs: u64,
        shutdown_flag: Arc<AtomicBool>,
    ) -> Self {
        let mut transport = Self::with_timeout(timeout_secs);
        transport.shutdown_flag = Some(shutdown_flag);
        transport
    }

    /// Create a new transport with a shutdown flag for graceful shutdown (reads timeout from environment).
    pub fn with_shutdown_flag(shutdown_flag: Arc<AtomicBool>) -> Self {
        let mut transport = Self::new();
        transport.shutdown_flag = Some(shutdown_flag);
        transport
    }
}

impl HttpTransport for ReqwestHttpTransport {
    fn post_json_with_headers(
        &self,
        url: &str,
        body: &[u8],
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse, ProviderTurnError> {
        validate_url_scheme(url)?;
        let mut request = self
            .client
            .post(url)
            .body(body.to_vec())
            .header("Content-Type", "application/json");
        for (key, value) in headers {
            validate_header_value(key, value)?;
            request = request.header(*key, *value);
        }
        let response = request.send().map_err(classify_reqwest_error)?;
        let status_code = response.status().as_u16();
        let mut response_body = Vec::new();
        response
            .take(MAX_PROVIDER_RESPONSE_SIZE)
            .read_to_end(&mut response_body)
            .map_err(|e| ProviderTurnError::Network(e.to_string()))?;
        Ok(HttpResponse {
            status_code,
            body: response_body,
        })
    }

    fn get_with_headers(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse, ProviderTurnError> {
        validate_url_scheme(url)?;
        let mut request = self.client.get(url);
        for (key, value) in headers {
            validate_header_value(key, value)?;
            request = request.header(*key, *value);
        }
        let response = request.send().map_err(classify_reqwest_error)?;
        let status_code = response.status().as_u16();
        let mut response_body = Vec::new();
        response
            .take(MAX_PROVIDER_RESPONSE_SIZE)
            .read_to_end(&mut response_body)
            .map_err(|e| ProviderTurnError::Network(e.to_string()))?;
        Ok(HttpResponse {
            status_code,
            body: response_body,
        })
    }

    fn stream_lines(
        &self,
        url: &str,
        body: &[u8],
        headers: &[(&str, &str)],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<(), ProviderTurnError> {
        validate_url_scheme(url)?;
        let mut request = self
            .streaming_client
            .post(url)
            .body(body.to_vec())
            .header("Content-Type", "application/json");
        for (key, value) in headers {
            validate_header_value(key, value)?;
            request = request.header(*key, *value);
        }
        let response = request.send().map_err(classify_reqwest_error)?;
        let reader = std::io::BufReader::new(response);
        use std::io::BufRead;
        for line in reader.lines() {
            if let Some(flag) = &self.shutdown_flag
                && flag.load(Ordering::Relaxed)
            {
                return Err(ProviderTurnError::Cancelled);
            }
            let line = line.map_err(|e| ProviderTurnError::Network(e.to_string()))?;
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                on_line(trimmed);
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Truncate and redact secrets from an error message.
///
/// 1. Truncates to 500 characters (D4-007: prevent information leakage)
/// 2. Redacts Authorization/Bearer/api_key patterns (D4-003: prevent API key leakage)
pub fn sanitize_error_message(message: &str) -> String {
    let truncated = if message.chars().count() > 500 {
        let cut: String = message.chars().take(500).collect();
        format!(
            "{}... [truncated, {} chars total]",
            cut,
            message.chars().count()
        )
    } else {
        message.to_string()
    };
    redact_secrets(&truncated)
}

/// Mask known secret patterns in a message string.
///
/// Replaces values following `Authorization:`, `Bearer`, `api_key:`,
/// `api-key:`, `x-api-key:`, and `apikey:` with `[REDACTED]`.
/// Follows the existing `api_key [REDACTED]` convention from `config/mod.rs`.
pub fn redact_secrets(message: &str) -> String {
    let mut result = String::with_capacity(message.len());
    for line in message.lines() {
        if !result.is_empty() {
            result.push('\n');
        }
        let lower = line.to_ascii_lowercase();
        if let Some(pos) = lower.find("authorization:") {
            let prefix_end = pos + "authorization:".len();
            result.push_str(&line[..prefix_end]);
            result.push_str(" [REDACTED]");
        } else if let Some(pos) = lower.find("bearer ") {
            let prefix_end = pos + "bearer ".len();
            result.push_str(&line[..prefix_end]);
            result.push_str("[REDACTED]");
        } else if let Some(pos) = lower.find("x-api-key:") {
            let prefix_end = pos + "x-api-key:".len();
            result.push_str(&line[..prefix_end]);
            result.push_str(" [REDACTED]");
        } else if let Some(pos) = lower.find("api-key:") {
            let prefix_end = pos + "api-key:".len();
            result.push_str(&line[..prefix_end]);
            result.push_str(" [REDACTED]");
        } else if let Some(pos) = lower.find("api_key:") {
            let prefix_end = pos + "api_key:".len();
            result.push_str(&line[..prefix_end]);
            result.push_str(" [REDACTED]");
        } else if let Some(pos) = lower.find("apikey:") {
            let prefix_end = pos + "apikey:".len();
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
    shutdown_flag: Option<Arc<AtomicBool>>,
}

impl<T: HttpTransport> RetryTransport<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            config: RetryConfig::default(),
            shutdown_flag: None,
        }
    }

    pub fn with_config(inner: T, config: RetryConfig) -> Self {
        Self {
            inner,
            config,
            shutdown_flag: None,
        }
    }

    /// Create a retry transport with a shutdown flag for graceful shutdown.
    pub fn with_shutdown_flag(inner: T, config: RetryConfig, flag: Arc<AtomicBool>) -> Self {
        Self {
            inner,
            config,
            shutdown_flag: Some(flag),
        }
    }

    fn is_shutdown(&self) -> bool {
        self.shutdown_flag
            .as_ref()
            .is_some_and(|f| f.load(Ordering::Relaxed))
    }

    /// Interruptible sleep that checks the shutdown flag every 100ms.
    /// Returns `true` if interrupted by shutdown.
    fn interruptible_sleep(&self, duration: std::time::Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < duration {
            if self.is_shutdown() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
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
                    if self.interruptible_sleep(std::time::Duration::from_millis(delay)) {
                        return Err(ProviderTurnError::Cancelled);
                    }
                    last_error = Some(err);
                }
                Err(err) => return Err(err),
            }
        }
        Err(last_error.expect("retry loop executed at least once"))
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
