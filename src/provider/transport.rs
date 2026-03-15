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
            return Err(ProviderTurnError::Backend(format!(
                "request failed with status {}: {}",
                response.status_code,
                body_text.trim()
            )));
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

/// Spawn curl in streaming mode (`-N`) and deliver each response line
/// to the callback as it arrives from the server.
fn curl_stream_lines(
    url: &str,
    body: &[u8],
    extra_headers: &[(&str, &str)],
    on_line: &mut dyn FnMut(&str),
) -> Result<(), ProviderTurnError> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "-N", "-X", "POST"])
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
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| ProviderTurnError::Backend(format!("failed to spawn curl: {err}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(body).map_err(|err| {
            ProviderTurnError::Backend(format!("failed to write to curl stdin: {err}"))
        })?;
    }

    if let Some(stdout) = child.stdout.take() {
        let reader = std::io::BufReader::new(stdout);
        use std::io::BufRead;
        for line in reader.lines() {
            let line = line.map_err(|err| {
                ProviderTurnError::Backend(format!("failed to read curl output: {err}"))
            })?;
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                on_line(trimmed);
            }
        }
    }

    let status = child.wait().map_err(|err| {
        ProviderTurnError::Backend(format!("curl process error: {err}"))
    })?;
    if !status.success() {
        return Err(ProviderTurnError::Backend(
            "curl streaming request failed".to_string(),
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
