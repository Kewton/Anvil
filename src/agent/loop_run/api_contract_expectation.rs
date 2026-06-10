//! Typed HTTP API contract expectations.
//!
//! The extractor is deliberately small and request-backed: it only emits an
//! expectation when an HTTP method and path are present in the request text.
//! Framework names such as FastAPI or Flask are not used as semantic triggers.

const MAX_API_EXPECTATIONS: usize = 6;
const MAX_FIELDS: usize = 8;
const MAX_FIELD_LEN: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    fn all() -> &'static [Self] {
        &[Self::Get, Self::Post, Self::Put, Self::Patch, Self::Delete]
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ApiContractExpectation {
    pub(super) method: HttpMethod,
    pub(super) path: String,
    pub(super) request_json_fields: Vec<String>,
    pub(super) expected_status: Option<u16>,
    pub(super) response_fields: Vec<String>,
}

impl ApiContractExpectation {
    pub(super) fn summary(&self) -> String {
        let mut parts = vec![
            format!("method={}", self.method.label()),
            format!("path={}", mask(&self.path)),
        ];
        if !self.request_json_fields.is_empty() {
            parts.push("request_body=json".to_string());
            parts.push("request_binding=json_body_object".to_string());
            parts.push(format!(
                "request_json_body_fields={}",
                self.request_json_fields
                    .iter()
                    .map(|field| mask(field))
                    .collect::<Vec<_>>()
                    .join("|")
            ));
        }
        match self.expected_status {
            Some(status) => parts.push(format!("expected_status={status}")),
            None => parts.push("expected_status=unspecified".to_string()),
        }
        if !self.response_fields.is_empty() {
            parts.push(format!(
                "response_fields={}",
                self.response_fields
                    .iter()
                    .map(|field| mask(field))
                    .collect::<Vec<_>>()
                    .join("|")
            ));
        }
        parts.join(",")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApiContractObservationKind {
    RequestSchemaMismatch,
    StatusMismatch,
    ResponseShapeMismatch,
    ApiContractMismatch,
}

impl ApiContractObservationKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::RequestSchemaMismatch => "request_schema_mismatch",
            Self::StatusMismatch => "status_mismatch",
            Self::ResponseShapeMismatch => "response_shape_mismatch",
            Self::ApiContractMismatch => "api_contract_mismatch",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApiRequestBindingIssue {
    JsonBodyFieldsNotBound,
}

impl ApiRequestBindingIssue {
    fn label(self) -> &'static str {
        match self {
            Self::JsonBodyFieldsNotBound => "json_body_fields_not_bound",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApiExpectedStatusPolicy {
    Unspecified,
    Explicit(u16),
}

impl ApiExpectedStatusPolicy {
    fn summary(self) -> String {
        match self {
            Self::Unspecified => "status_policy=unspecified".to_string(),
            Self::Explicit(status) => format!("status_policy=explicit:{status}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ApiStatusObservation {
    pub(super) policy: ApiExpectedStatusPolicy,
    pub(super) expected_from_diagnostic: Option<u16>,
    pub(super) observed_status: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ApiContractObservation {
    pub(super) kind: ApiContractObservationKind,
    pub(super) method: HttpMethod,
    pub(super) path: String,
    pub(super) request_json_fields: Vec<String>,
    pub(super) request_binding_issue: Option<ApiRequestBindingIssue>,
    pub(super) status: Option<ApiStatusObservation>,
}

impl ApiContractObservation {
    fn summary(&self) -> String {
        let mut parts = vec![
            format!("method={}", self.method.label()),
            format!("path={}", mask(&self.path)),
        ];
        if !self.request_json_fields.is_empty() {
            parts.push(format!(
                "request_json_body_fields={}",
                self.request_json_fields
                    .iter()
                    .map(|field| mask(field))
                    .collect::<Vec<_>>()
                    .join("|")
            ));
        }
        if let Some(issue) = self.request_binding_issue {
            parts.push(format!("request_binding_issue={}", issue.label()));
        }
        if let Some(status) = &self.status {
            parts.push(status.policy.summary());
            if let Some(expected) = status.expected_from_diagnostic {
                parts.push(format!("diagnostic_expected_status={expected}"));
            }
            if let Some(observed) = status.observed_status {
                parts.push(format!("observed_status={observed}"));
            }
        }
        parts.join(",")
    }

    fn repair_hint(&self) -> Option<String> {
        match self.kind {
            ApiContractObservationKind::RequestSchemaMismatch
                if self.request_binding_issue
                    == Some(ApiRequestBindingIssue::JsonBodyFieldsNotBound) =>
            {
                Some(
                    "bind_declared_fields_from_json_request_body_object_not_query_or_form_params"
                        .to_string(),
                )
            }
            ApiContractObservationKind::StatusMismatch => {
                match self.status.as_ref().map(|status| status.policy) {
                    Some(ApiExpectedStatusPolicy::Unspecified) => Some(
                        "do_not_invent_exact_http_status_when_expected_status_unspecified"
                            .to_string(),
                    ),
                    Some(ApiExpectedStatusPolicy::Explicit(status)) => {
                        Some(format!("honor_explicit_http_status_{status}"))
                    }
                    None => None,
                }
            }
            _ => None,
        }
    }
}

pub(super) fn extract_api_contract_expectations(request: &str) -> Vec<ApiContractExpectation> {
    let lower = request.to_ascii_lowercase();
    let mut occurrences = method_path_occurrences(&lower);
    occurrences.sort_by_key(|occurrence| occurrence.method_start);
    occurrences.dedup_by(|a, b| a.method == b.method && a.path == b.path);

    let mut expectations = Vec::new();
    for (idx, occurrence) in occurrences.iter().enumerate().take(MAX_API_EXPECTATIONS) {
        let next_start = occurrences
            .iter()
            .skip(idx + 1)
            .map(|next| next.method_start)
            .find(|next_start| *next_start > occurrence.method_start)
            .unwrap_or(lower.len());
        let clause_end = next_clause_boundary(&lower, occurrence.path_end, next_start);
        let clause = &lower[occurrence.method_start..clause_end];
        let request_json_fields = request_json_fields_from_clause(clause);
        let response_fields = response_fields_from_clause(clause, &request_json_fields);
        expectations.push(ApiContractExpectation {
            method: occurrence.method,
            path: occurrence.path.clone(),
            request_json_fields,
            expected_status: expected_status_from_clause(clause),
            response_fields,
        });
    }
    expectations
}

pub(super) fn api_contract_summary(expectations: &[ApiContractExpectation]) -> Option<String> {
    if expectations.is_empty() {
        return None;
    }
    Some(
        expectations
            .iter()
            .take(MAX_API_EXPECTATIONS)
            .map(ApiContractExpectation::summary)
            .collect::<Vec<_>>()
            .join("|"),
    )
}

pub(super) fn api_contract_delta_summary(
    expectations: &[ApiContractExpectation],
    diagnostic: &str,
) -> Option<String> {
    let expected = api_contract_summary(expectations)?;
    let observation = observe_api_contract_mismatch(expectations, diagnostic)?;
    let hint = observation
        .repair_hint()
        .map(|hint| format!("; repair_hint={hint}"))
        .unwrap_or_default();
    Some(format!(
        "kind={}; expected={expected}; observed={}{hint}",
        observation.kind.label(),
        observation.summary(),
    ))
}

pub(super) fn api_contract_payload_value_from_request(
    request: &str,
    diagnostic: &str,
) -> serde_json::Value {
    let expectations = extract_api_contract_expectations(request);
    api_contract_payload_value(&expectations, diagnostic)
}

pub(super) fn api_contract_payload_value(
    expectations: &[ApiContractExpectation],
    diagnostic: &str,
) -> serde_json::Value {
    let Some(expectations_summary) = api_contract_summary(expectations) else {
        return serde_json::Value::Null;
    };
    serde_json::json!({
        "expectations": expectations_summary,
        "observation": api_contract_delta_summary(expectations, diagnostic),
    })
}

pub(super) fn observe_api_contract_mismatch(
    expectations: &[ApiContractExpectation],
    diagnostic: &str,
) -> Option<ApiContractObservation> {
    let lower = diagnostic.to_ascii_lowercase();
    let kind = api_observation_kind(&lower);
    let expectation = select_api_observation_expectation(expectations, kind)?;
    let request_binding_issue = (kind == ApiContractObservationKind::RequestSchemaMismatch
        && !expectation.request_json_fields.is_empty())
    .then_some(ApiRequestBindingIssue::JsonBodyFieldsNotBound);
    let status = observe_api_status(expectations, diagnostic, kind);
    Some(ApiContractObservation {
        kind,
        method: expectation.method,
        path: expectation.path.clone(),
        request_json_fields: expectation.request_json_fields.clone(),
        request_binding_issue,
        status,
    })
}

fn api_observation_kind(lower_diagnostic: &str) -> ApiContractObservationKind {
    if lower_diagnostic.contains("422") || lower_diagnostic.contains("unprocessable entity") {
        ApiContractObservationKind::RequestSchemaMismatch
    } else if lower_diagnostic.contains("status") || lower_diagnostic.contains("status_code") {
        ApiContractObservationKind::StatusMismatch
    } else if lower_diagnostic.contains("response") || lower_diagnostic.contains("json") {
        ApiContractObservationKind::ResponseShapeMismatch
    } else {
        ApiContractObservationKind::ApiContractMismatch
    }
}

fn select_api_observation_expectation(
    expectations: &[ApiContractExpectation],
    kind: ApiContractObservationKind,
) -> Option<&ApiContractExpectation> {
    match kind {
        ApiContractObservationKind::RequestSchemaMismatch => expectations
            .iter()
            .find(|expectation| !expectation.request_json_fields.is_empty())
            .or_else(|| expectations.first()),
        ApiContractObservationKind::StatusMismatch => expectations
            .iter()
            .find(|expectation| expectation.expected_status.is_some())
            .or_else(|| expectations.first()),
        ApiContractObservationKind::ResponseShapeMismatch => expectations
            .iter()
            .find(|expectation| !expectation.response_fields.is_empty())
            .or_else(|| expectations.first()),
        ApiContractObservationKind::ApiContractMismatch => expectations.first(),
    }
}

fn observe_api_status(
    expectations: &[ApiContractExpectation],
    diagnostic: &str,
    kind: ApiContractObservationKind,
) -> Option<ApiStatusObservation> {
    let codes = status_codes_from_text(diagnostic);
    if kind != ApiContractObservationKind::StatusMismatch && !codes.contains(&422) {
        return None;
    }
    let policy = expectations
        .iter()
        .find_map(|expectation| expectation.expected_status)
        .map(ApiExpectedStatusPolicy::Explicit)
        .unwrap_or(ApiExpectedStatusPolicy::Unspecified);
    Some(ApiStatusObservation {
        policy,
        expected_from_diagnostic: codes.first().copied(),
        observed_status: codes
            .get(1)
            .copied()
            .or_else(|| codes.contains(&422).then_some(422)),
    })
}

fn status_codes_from_text(text: &str) -> Vec<u16> {
    let mut codes = Vec::new();
    for token in text
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| token.len() == 3)
    {
        if let Ok(status) = token.parse::<u16>()
            && (100..=599).contains(&status)
            && !codes.contains(&status)
        {
            codes.push(status);
        }
    }
    codes
}

pub(super) fn api_contract_artifact_directed_context(
    expectations: &[ApiContractExpectation],
) -> Option<String> {
    let summary = api_contract_summary(expectations)?;
    let summary = super::task_contract::mask_and_cap_recovery_field(&summary);
    Some(format!(
        "Typed API contract context: api_contracts={summary}. If api_contracts contains request_body=json, implement request_json_body_fields as JSON request-body object fields, not query or form parameters."
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MethodPathOccurrence {
    method: HttpMethod,
    method_start: usize,
    path: String,
    path_end: usize,
}

fn method_path_occurrences(lower: &str) -> Vec<MethodPathOccurrence> {
    let mut occurrences = Vec::new();
    for method in HttpMethod::all() {
        let needle = method.label().to_ascii_lowercase();
        let mut search_start = 0;
        while let Some(offset) = lower[search_start..].find(&needle) {
            let method_start = search_start + offset;
            let method_end = method_start + needle.len();
            search_start = method_end;
            if !is_word_boundary(lower, method_start, method_end) {
                continue;
            }
            let Some((path, path_end)) = path_after_method(lower, method_end) else {
                continue;
            };
            occurrences.push(MethodPathOccurrence {
                method: *method,
                method_start,
                path,
                path_end,
            });
        }
    }
    occurrences
}

fn path_after_method(lower: &str, from: usize) -> Option<(String, usize)> {
    let slash = lower[from..].find('/')? + from;
    if lower[from..slash]
        .chars()
        .any(|ch| !ch.is_ascii_whitespace() && !matches!(ch, ':' | '`' | '"' | '\''))
    {
        return None;
    }
    let mut end = slash;
    for (idx, ch) in lower[slash..].char_indices() {
        if idx == 0 {
            end = slash + ch.len_utf8();
            continue;
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '/' | '-' | '_' | '{' | '}' | ':') {
            end = slash + idx + ch.len_utf8();
        } else {
            break;
        }
    }
    let path = lower[slash..end].trim_matches(['`', '"', '\'']).to_string();
    (!path.is_empty()).then_some((path, end))
}

fn next_clause_boundary(lower: &str, from: usize, next_method_start: usize) -> usize {
    let mut end = next_method_start;
    for boundary in ['.', ';', '\n'] {
        if let Some(offset) = lower[from..next_method_start].find(boundary) {
            end = end.min(from + offset);
        }
    }
    end
}

fn request_json_fields_from_clause(clause: &str) -> Vec<String> {
    for marker in [
        "json with",
        "json body with",
        "body with",
        "accepting json with",
    ] {
        if let Some(start) = clause.find(marker) {
            let after = &clause[start + marker.len()..];
            let end = first_marker(after, &[" returning", ", returning", ";", "."])
                .unwrap_or(after.len());
            return extract_field_names(&after[..end]);
        }
    }
    Vec::new()
}

fn response_fields_from_clause(clause: &str, request_json_fields: &[String]) -> Vec<String> {
    let Some(start) = clause.find("return") else {
        return Vec::new();
    };
    let response_clause = &clause[start..];
    if response_clause.contains("empty list") || response_clause.contains("empty array") {
        return Vec::new();
    }
    let mut fields = extract_field_names(response_clause);
    if response_clause.contains("created") {
        for field in request_json_fields {
            push_field(&mut fields, field);
        }
    }
    fields
}

fn expected_status_from_clause(clause: &str) -> Option<u16> {
    for token in clause
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| token.len() == 3)
    {
        if let Ok(status) = token.parse::<u16>()
            && (100..=599).contains(&status)
        {
            return Some(status);
        }
    }
    None
}

fn extract_field_names(text: &str) -> Vec<String> {
    let mut fields = Vec::new();
    for token in text.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_') {
        let token = token.trim_matches('_');
        if is_field_token(token) {
            push_field(&mut fields, token);
        }
        if fields.len() >= MAX_FIELDS {
            break;
        }
    }
    fields
}

fn push_field(fields: &mut Vec<String>, value: &str) {
    let value = value.trim().to_string();
    if !value.is_empty() && !fields.contains(&value) && fields.len() < MAX_FIELDS {
        fields.push(value);
    }
}

fn is_field_token(token: &str) -> bool {
    if token.is_empty()
        || token.len() > MAX_FIELD_LEN
        || !token
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
    {
        return false;
    }
    !matches!(
        token,
        "a" | "an"
            | "and"
            | "api"
            | "array"
            | "code"
            | "created"
            | "empty"
            | "field"
            | "fields"
            | "get"
            | "http"
            | "json"
            | "list"
            | "note"
            | "object"
            | "or"
            | "post"
            | "put"
            | "patch"
            | "delete"
            | "request"
            | "response"
            | "return"
            | "returning"
            | "returns"
            | "status"
            | "the"
            | "with"
    )
}

fn first_marker(haystack: &str, markers: &[&str]) -> Option<usize> {
    markers
        .iter()
        .filter_map(|marker| haystack.find(marker))
        .min()
}

fn is_word_boundary(text: &str, start: usize, end: usize) -> bool {
    let before_ok = text[..start]
        .chars()
        .next_back()
        .is_none_or(|ch| !ch.is_ascii_alphanumeric() && ch != '_');
    let after_ok = text[end..]
        .chars()
        .next()
        .is_none_or(|ch| !ch.is_ascii_alphanumeric() && ch != '_');
    before_ok && after_ok
}

fn mask(value: &str) -> String {
    super::task_contract::mask_and_cap_recovery_field(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_post_request_and_response_fields() {
        let expectations = extract_api_contract_expectations(
            "Implement POST /notes accepting JSON with title and body, returning the created note with id=1.",
        );

        assert_eq!(expectations.len(), 1);
        let post = &expectations[0];
        assert_eq!(post.method, HttpMethod::Post);
        assert_eq!(post.path, "/notes");
        assert_eq!(post.request_json_fields, vec!["title", "body"]);
        assert_eq!(post.response_fields, vec!["id", "title", "body"]);
        assert_eq!(post.expected_status, None);
    }

    #[test]
    fn extracts_multiple_http_methods_without_framework_trigger() {
        let expectations = extract_api_contract_expectations(
            "Build an HTTP API: GET /notes returning an empty list and POST /notes accepting JSON with title and body.",
        );

        assert_eq!(expectations.len(), 2);
        assert_eq!(expectations[0].method, HttpMethod::Get);
        assert_eq!(expectations[0].path, "/notes");
        assert!(expectations[0].response_fields.is_empty());
        assert_eq!(expectations[1].method, HttpMethod::Post);
        assert_eq!(expectations[1].request_json_fields, vec!["title", "body"]);
    }

    #[test]
    fn extracts_explicit_status_only_when_present() {
        let expectations = extract_api_contract_expectations(
            "POST /items accepting JSON with name and price returns status 201 with id.",
        );

        assert_eq!(expectations[0].expected_status, Some(201));
    }

    #[test]
    fn projects_unspecified_status_when_request_does_not_declare_one() {
        let expectations = extract_api_contract_expectations(
            "POST /notes accepting JSON with title and body returning the created note with id.",
        );
        let summary = api_contract_summary(&expectations).expect("summary");

        assert!(summary.contains("expected_status=unspecified"), "{summary}");
        assert!(!summary.contains("expected_status=201"), "{summary}");
    }

    #[test]
    fn emits_request_schema_delta_for_422() {
        let expectations = extract_api_contract_expectations(
            "POST /notes accepting JSON with title and body returning id.",
        );
        let delta = api_contract_delta_summary(
            &expectations,
            "AssertionError: expected 200 but got 422 Unprocessable Entity",
        )
        .expect("api delta");

        assert!(delta.contains("kind=request_schema_mismatch"), "{delta}");
        assert!(
            delta.contains("request_json_body_fields=title|body"),
            "{delta}"
        );
        assert!(
            delta.contains(
                "bind_declared_fields_from_json_request_body_object_not_query_or_form_params"
            ),
            "{delta}"
        );
    }

    #[test]
    fn observes_post_body_field_binding_issue() {
        let expectations = extract_api_contract_expectations(
            "POST /notes accepting JSON with title and body returning id.",
        );
        let observation = observe_api_contract_mismatch(
            &expectations,
            "AssertionError: POST /notes returned 422 Unprocessable Entity",
        )
        .expect("api observation");

        assert_eq!(
            observation.kind,
            ApiContractObservationKind::RequestSchemaMismatch
        );
        assert_eq!(observation.method, HttpMethod::Post);
        assert_eq!(observation.path, "/notes");
        assert_eq!(observation.request_json_fields, vec!["title", "body"]);
        assert_eq!(
            observation.request_binding_issue,
            Some(ApiRequestBindingIssue::JsonBodyFieldsNotBound)
        );
        assert_eq!(
            observation
                .status
                .as_ref()
                .and_then(|status| status.observed_status),
            Some(422)
        );
    }

    #[test]
    fn observes_unspecified_status_without_forcing_exact_created_status() {
        let expectations = extract_api_contract_expectations(
            "POST /notes accepting JSON with title and body returning the created note with id.",
        );
        let observation = observe_api_contract_mismatch(
            &expectations,
            "AssertionError: expected status 201 but got 200",
        )
        .expect("api observation");

        assert_eq!(observation.kind, ApiContractObservationKind::StatusMismatch);
        let status = observation.status.as_ref().expect("status observation");
        assert_eq!(status.policy, ApiExpectedStatusPolicy::Unspecified);
        assert_eq!(status.expected_from_diagnostic, Some(201));
        assert_eq!(status.observed_status, Some(200));
        assert_eq!(
            observation.repair_hint().as_deref(),
            Some("do_not_invent_exact_http_status_when_expected_status_unspecified")
        );
    }

    #[test]
    fn observes_explicit_status_policy_when_declared() {
        let expectations = extract_api_contract_expectations(
            "POST /items accepting JSON with name and price returns status 201 with id.",
        );
        let observation = observe_api_contract_mismatch(
            &expectations,
            "AssertionError: expected status 201 but got 200",
        )
        .expect("api observation");

        assert_eq!(observation.kind, ApiContractObservationKind::StatusMismatch);
        assert_eq!(
            observation.status.as_ref().map(|status| status.policy),
            Some(ApiExpectedStatusPolicy::Explicit(201))
        );
        assert_eq!(
            observation.repair_hint().as_deref(),
            Some("honor_explicit_http_status_201")
        );
    }

    #[test]
    fn payload_value_from_request_carries_typed_observation() {
        let payload = api_contract_payload_value_from_request(
            "Implement POST /notes accepting JSON with title and body returning id.",
            "AssertionError: POST /notes returned 422 Unprocessable Entity",
        );

        assert_eq!(
            payload["observation"].as_str().unwrap_or_default(),
            "kind=request_schema_mismatch; expected=method=POST,path=/notes,request_body=json,request_binding=json_body_object,request_json_body_fields=title|body,expected_status=unspecified,response_fields=id; observed=method=POST,path=/notes,request_json_body_fields=title|body,request_binding_issue=json_body_fields_not_bound,status_policy=unspecified,diagnostic_expected_status=422,observed_status=422; repair_hint=bind_declared_fields_from_json_request_body_object_not_query_or_form_params"
        );
    }

    #[test]
    fn emits_status_drift_delta_when_status_was_not_declared() {
        let expectations = extract_api_contract_expectations(
            "POST /notes accepting JSON with title and body returning the created note with id.",
        );
        let delta = api_contract_delta_summary(
            &expectations,
            "AssertionError: expected status 201 but got 200",
        )
        .expect("api delta");

        assert!(delta.contains("kind=status_mismatch"), "{delta}");
        assert!(delta.contains("expected_status=unspecified"), "{delta}");
        assert!(
            delta.contains("do_not_invent_exact_http_status_when_expected_status_unspecified"),
            "{delta}"
        );
        assert!(
            delta.contains("observed=method=POST,path=/notes"),
            "{delta}"
        );
    }

    #[test]
    fn ignores_non_http_api_mentions() {
        assert!(
            extract_api_contract_expectations("Document the API usage in README.md").is_empty()
        );
    }
}
