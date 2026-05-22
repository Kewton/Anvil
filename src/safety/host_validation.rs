use reqwest::Url;

pub fn validate_localhost_url(raw: String, label: &str) -> Result<String, String> {
    let url = Url::parse(&raw).map_err(|err| format!("invalid {label}: {err}"))?;
    if url.username() != "" || url.password().is_some() {
        return Err(format!("{label} must not include credentials"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| format!("{label} must include a host"))?;
    let allowed = matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]");
    if !allowed {
        return Err(format!("{label} must point to localhost"));
    }
    match url.scheme() {
        "http" | "https" => {}
        _ => return Err(format!("{label} must use http or https")),
    }
    Ok(raw.trim_end_matches('/').to_string())
}
