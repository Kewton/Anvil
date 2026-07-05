use std::time::Duration;

use reqwest::blocking::Client;

pub(crate) const PROVIDER_CONNECT_TIMEOUT_SECS: u64 = 10;
pub(crate) const PROVIDER_REQUEST_TIMEOUT_MARGIN_SECS: u64 = 2;

pub(crate) fn request_timeout_secs(chat_timeout_secs: u64) -> u64 {
    chat_timeout_secs.saturating_add(PROVIDER_REQUEST_TIMEOUT_MARGIN_SECS)
}

pub(crate) fn blocking_client(chat_timeout_secs: u64) -> Result<Client, reqwest::Error> {
    let request_timeout_secs = request_timeout_secs(chat_timeout_secs);
    Client::builder()
        .connect_timeout(Duration::from_secs(
            PROVIDER_CONNECT_TIMEOUT_SECS.min(request_timeout_secs),
        ))
        .timeout(Duration::from_secs(request_timeout_secs))
        .build()
}

pub(crate) fn provider_turn_timeout_message(
    provider: &str,
    operation: &str,
    chat_timeout_secs: u64,
    err: impl std::fmt::Display,
) -> String {
    format!(
        "provider_turn_timeout: {provider} {operation} timed out after {chat_timeout_secs}s \
         (transport deadline {}s): {err}",
        request_timeout_secs(chat_timeout_secs)
    )
}
