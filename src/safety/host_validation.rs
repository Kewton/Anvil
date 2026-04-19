use reqwest::Url;

pub fn validate_localhost_url(raw: String) -> Result<String, String> {
    let url = Url::parse(&raw).map_err(|err| format!("invalid ollama host: {err}"))?;
    if url.username() != "" || url.password().is_some() {
        return Err("ollama host must not include credentials".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "ollama host must include a host".to_string())?;
    let allowed = matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]");
    if !allowed {
        return Err("ollama host must point to localhost".to_string());
    }
    match url.scheme() {
        "http" | "https" => {}
        _ => return Err("ollama host must use http or https".to_string()),
    }
    Ok(raw.trim_end_matches('/').to_string())
}
