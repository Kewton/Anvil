use super::task_contract::StructuredColumnPolicy;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StructuredDataObservation {
    pub(super) path: String,
    pub(super) required_columns: Vec<String>,
    pub(super) observed_columns: Vec<String>,
    pub(super) missing_columns: Vec<String>,
    pub(super) extra_columns: Vec<String>,
    pub(super) expected_rows: Vec<Vec<String>>,
    pub(super) observed_rows: Option<Vec<Vec<String>>>,
    pub(super) failure: Option<StructuredDataObservationFailure>,
}

impl StructuredDataObservation {
    pub(super) fn diagnostic_message(&self) -> Option<String> {
        let failure = self.failure.as_ref()?;
        Some(match failure {
            StructuredDataObservationFailure::ExactColumnsMismatch => format!(
                "structured data columns must match exactly: expected {}; observed {}; remove extra fields and add missing required fields",
                display_schema_columns(&self.required_columns),
                display_schema_columns(&self.observed_columns)
            ),
            StructuredDataObservationFailure::ParseError(message) => (*message).to_string(),
            StructuredDataObservationFailure::Empty => {
                "structured data evidence is empty".to_string()
            }
            StructuredDataObservationFailure::MissingRequiredColumns => format!(
                "structured data is missing required columns: {}; observed columns: {}; add all required columns",
                display_schema_columns(&self.missing_columns),
                display_schema_columns(&self.observed_columns)
            ),
            StructuredDataObservationFailure::ExpectedRowsRequireDelimited => {
                "structured data expected rows require CSV or TSV parse evidence".to_string()
            }
            StructuredDataObservationFailure::ExpectedRowsNoRecords => {
                "structured data expected rows have no parse-ready records".to_string()
            }
            StructuredDataObservationFailure::ExpectedRowsColumnMismatch => format!(
                "structured data columns must match exactly for expected rows: expected {}; observed {}",
                display_schema_columns(&self.required_columns),
                display_schema_columns(&self.observed_columns)
            ),
            StructuredDataObservationFailure::ExpectedRowsCellCount { expected_cells } => {
                format!("structured data expected rows require {expected_cells} cells per row")
            }
            StructuredDataObservationFailure::ExpectedRowsMismatch => format!(
                "structured data expected rows do not match: expected rows {}; observed rows {}",
                display_schema_rows(&self.expected_rows),
                display_schema_rows(self.observed_rows.as_deref().unwrap_or_default())
            ),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StructuredDataObservationFailure {
    ExactColumnsMismatch,
    ParseError(&'static str),
    Empty,
    MissingRequiredColumns,
    ExpectedRowsRequireDelimited,
    ExpectedRowsNoRecords,
    ExpectedRowsColumnMismatch,
    ExpectedRowsCellCount { expected_cells: usize },
    ExpectedRowsMismatch,
}

pub(super) fn observe_structured_data_schema(
    path: &str,
    excerpt: &str,
    required_columns: &[String],
    expected_rows: &[Vec<String>],
    column_policy: StructuredColumnPolicy,
) -> StructuredDataObservation {
    let mut observation = StructuredDataObservation {
        path: path.to_string(),
        required_columns: required_columns.to_vec(),
        observed_columns: Vec::new(),
        missing_columns: Vec::new(),
        extra_columns: Vec::new(),
        expected_rows: expected_rows.to_vec(),
        observed_rows: None,
        failure: None,
    };

    if let Some((observed_columns, missing_columns, extra_columns)) =
        exact_columns_mismatch(path, excerpt, required_columns, column_policy)
    {
        observation.observed_columns = observed_columns;
        observation.missing_columns = missing_columns;
        observation.extra_columns = extra_columns;
        observation.failure = Some(StructuredDataObservationFailure::ExactColumnsMismatch);
        return observation;
    }

    if let Some(message) = structured_data_parse_error(path, excerpt) {
        observation.failure = Some(StructuredDataObservationFailure::ParseError(message));
        return observation;
    }

    if excerpt.trim().is_empty() {
        observation.failure = Some(StructuredDataObservationFailure::Empty);
        return observation;
    }

    observation.observed_columns = observed_data_columns(Some(path), excerpt, required_columns);
    observation.missing_columns =
        missing_required_columns(required_columns, &observation.observed_columns);
    if !observation.missing_columns.is_empty() {
        observation.failure = Some(StructuredDataObservationFailure::MissingRequiredColumns);
        return observation;
    }

    if let Some(failure) = observe_expected_rows(
        path,
        excerpt,
        required_columns,
        expected_rows,
        &mut observation,
    ) {
        observation.failure = Some(failure);
    }
    observation
}

pub(super) fn structured_data_exact_columns_diagnostic(
    path: &str,
    excerpt: &str,
    required_columns: &[String],
    column_policy: StructuredColumnPolicy,
) -> Option<String> {
    exact_columns_mismatch(path, excerpt, required_columns, column_policy).map(|(observed, _, _)| {
        format!(
            "structured data columns must match exactly: expected {}; observed {}; remove extra fields and add missing required fields",
            display_schema_columns(required_columns),
            display_schema_columns(&observed)
        )
    })
}

fn exact_columns_mismatch(
    path: &str,
    excerpt: &str,
    required_columns: &[String],
    column_policy: StructuredColumnPolicy,
) -> Option<(Vec<String>, Vec<String>, Vec<String>)> {
    if required_columns.is_empty() {
        return None;
    }
    let json_object = path_has_extension(Some(path), "json");
    if !json_object && column_policy != StructuredColumnPolicy::Exact {
        return None;
    }
    let observed = if json_object {
        let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(excerpt)
        else {
            return None;
        };
        sorted_unique(map.keys().cloned().collect())
    } else {
        sorted_unique(observed_data_columns(Some(path), excerpt, required_columns))
    };
    let required = sorted_unique(required_columns.to_vec());
    if observed == required {
        return None;
    }
    let missing = missing_required_columns(&required, &observed);
    let extra = observed
        .iter()
        .filter(|column| !required.iter().any(|required| required == *column))
        .cloned()
        .collect();
    Some((observed, missing, extra))
}

fn observe_expected_rows(
    path: &str,
    excerpt: &str,
    required_columns: &[String],
    expected_rows: &[Vec<String>],
    observation: &mut StructuredDataObservation,
) -> Option<StructuredDataObservationFailure> {
    if expected_rows.is_empty() {
        return None;
    }
    let Some(delimiter) = delimited_data_delimiter(path) else {
        return Some(StructuredDataObservationFailure::ExpectedRowsRequireDelimited);
    };
    let Some((observed_columns, observed_rows)) = delimited_table(excerpt, delimiter) else {
        return Some(StructuredDataObservationFailure::ExpectedRowsNoRecords);
    };
    observation.observed_columns = observed_columns;
    observation.observed_rows = Some(observed_rows);
    if observation.observed_columns != required_columns {
        return Some(StructuredDataObservationFailure::ExpectedRowsColumnMismatch);
    }
    if observation
        .observed_rows
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|row| row.len() != required_columns.len())
    {
        return Some(StructuredDataObservationFailure::ExpectedRowsCellCount {
            expected_cells: required_columns.len(),
        });
    }
    let expected = normalized_row_set(expected_rows);
    let observed = normalized_row_set(observation.observed_rows.as_deref().unwrap_or_default());
    (expected != observed).then_some(StructuredDataObservationFailure::ExpectedRowsMismatch)
}

fn missing_required_columns(
    required_columns: &[String],
    observed_columns: &[String],
) -> Vec<String> {
    required_columns
        .iter()
        .filter(|column| !observed_columns.iter().any(|seen| seen == *column))
        .cloned()
        .collect()
}

fn delimited_data_delimiter(path: &str) -> Option<char> {
    match path_extension_lower(path).as_deref() {
        Some("csv") => Some(','),
        Some("tsv") => Some('\t'),
        _ => None,
    }
}

fn delimited_table(excerpt: &str, delimiter: char) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    let mut lines = excerpt.lines().filter(|line| !line.trim().is_empty());
    let header = lines
        .next()?
        .split(delimiter)
        .map(clean_data_column)
        .collect::<Vec<_>>();
    if header.is_empty() {
        return None;
    }
    let rows = lines
        .map(|line| {
            line.split(delimiter)
                .map(clean_data_column)
                .collect::<Vec<_>>()
        })
        .filter(|row| row.iter().any(|cell| !cell.is_empty()))
        .collect::<Vec<_>>();
    Some((header, rows))
}

fn normalized_row_set(rows: &[Vec<String>]) -> Vec<String> {
    let mut normalized = rows
        .iter()
        .map(|row| row.join("\u{1f}"))
        .collect::<Vec<_>>();
    normalized.sort();
    normalized
}

fn display_schema_rows(rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return "(none)".to_string();
    }
    rows.iter()
        .map(|row| {
            row.iter()
                .map(|cell| crate::session::feedback::mask_secrets(cell))
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<Vec<_>>()
        .join("; ")
}

pub(super) fn display_schema_columns(columns: &[String]) -> String {
    if columns.is_empty() {
        return "(none)".to_string();
    }
    columns
        .iter()
        .map(|column| crate::session::feedback::mask_secrets(column))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn structured_data_parse_error(path: &str, excerpt: &str) -> Option<&'static str> {
    match path_extension_lower(path).as_deref() {
        Some("json") => serde_json::from_str::<serde_json::Value>(excerpt)
            .is_err()
            .then_some("JSON data artifact is not parse-ready"),
        Some("jsonl") | Some("ndjson") => excerpt
            .lines()
            .filter(|line| !line.trim().is_empty())
            .any(|line| serde_json::from_str::<serde_json::Value>(line).is_err())
            .then_some("JSONL data artifact contains a non-parseable record"),
        Some("csv") => delimited_header_columns(excerpt, ',')
            .is_empty()
            .then_some("CSV data artifact is missing a parse-ready header"),
        Some("tsv") => delimited_header_columns(excerpt, '\t')
            .is_empty()
            .then_some("TSV data artifact is missing a parse-ready header"),
        _ => None,
    }
}

pub(super) fn accept_tier_format_is_parse_checkable(path: Option<&str>) -> bool {
    let Some(path) = path else {
        return true;
    };
    match path_extension_lower(path).as_deref() {
        None => true,
        Some("csv" | "tsv" | "json" | "jsonl" | "ndjson") => true,
        Some(_) => false,
    }
}

pub(super) fn path_has_extension(path: Option<&str>, expected: &str) -> bool {
    path.and_then(path_extension_lower)
        .is_some_and(|ext| ext == expected)
}

fn path_extension_lower(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
}

pub(super) fn observed_data_columns(
    path: Option<&str>,
    excerpt: &str,
    required_columns: &[String],
) -> Vec<String> {
    let normalized_ext = path.and_then(path_extension_lower);
    let columns = match normalized_ext.as_deref() {
        Some("tsv") => delimited_header_columns(excerpt, '\t'),
        Some("csv") => delimited_header_columns(excerpt, ','),
        Some("json") => json_columns(excerpt),
        Some("jsonl") | Some("ndjson") => jsonl_columns(excerpt),
        _ => generic_data_columns(excerpt, required_columns),
    };
    sorted_unique(columns)
}

fn delimited_header_columns(excerpt: &str, delimiter: char) -> Vec<String> {
    excerpt
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| {
            line.split(delimiter)
                .map(clean_data_column)
                .filter(|column| !column.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn json_columns(excerpt: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(excerpt) else {
        return Vec::new();
    };
    value_columns(&value)
}

fn jsonl_columns(excerpt: &str) -> Vec<String> {
    let mut columns = Vec::new();
    for line in excerpt.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            return Vec::new();
        };
        columns.extend(value_columns(&value));
    }
    columns
}

fn value_columns(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        serde_json::Value::Array(items) => items.iter().flat_map(value_columns).collect(),
        _ => Vec::new(),
    }
}

fn generic_data_columns(excerpt: &str, required_columns: &[String]) -> Vec<String> {
    if excerpt.trim().is_empty() {
        return Vec::new();
    }
    let mut columns = required_columns
        .iter()
        .filter(|column| excerpt.contains(column.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if columns.is_empty() && required_columns.is_empty() {
        columns.push("data".to_string());
    }
    columns
}

fn clean_data_column(raw: &str) -> String {
    raw.trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '`'))
        .trim()
        .to_string()
}

pub(super) fn sorted_unique(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    #[test]
    fn observation_passes_required_columns() {
        let observation = observe_structured_data_schema(
            "output.csv",
            "id,total\n1,10\n",
            &columns(&["id", "total"]),
            &[],
            StructuredColumnPolicy::RequiredOnly,
        );

        assert_eq!(observation.failure, None);
        assert_eq!(observation.observed_columns, columns(&["id", "total"]));
    }

    #[test]
    fn observation_fails_missing_columns() {
        let observation = observe_structured_data_schema(
            "output.csv",
            "id,amount\n1,10\n",
            &columns(&["id", "total"]),
            &[],
            StructuredColumnPolicy::RequiredOnly,
        );

        assert_eq!(
            observation.failure,
            Some(StructuredDataObservationFailure::MissingRequiredColumns)
        );
        assert_eq!(observation.missing_columns, columns(&["total"]));
        assert!(
            observation
                .diagnostic_message()
                .expect("diagnostic")
                .contains("missing required columns: total")
        );
    }

    #[test]
    fn observation_fails_extra_columns_only_when_exact_policy_is_explicit() {
        let required = columns(&["id", "total"]);
        let excerpt = "id,total,duplicate\n1,10,10\n";
        let required_only = observe_structured_data_schema(
            "output.csv",
            excerpt,
            &required,
            &[],
            StructuredColumnPolicy::RequiredOnly,
        );
        let exact = observe_structured_data_schema(
            "output.csv",
            excerpt,
            &required,
            &[],
            StructuredColumnPolicy::Exact,
        );

        assert_eq!(required_only.failure, None);
        assert_eq!(
            exact.failure,
            Some(StructuredDataObservationFailure::ExactColumnsMismatch)
        );
        assert_eq!(exact.extra_columns, columns(&["duplicate"]));
    }

    #[test]
    fn observation_fails_expected_row_mismatch() {
        let observation = observe_structured_data_schema(
            "output.csv",
            "id,total\n1,10\n2,30\n",
            &columns(&["id", "total"]),
            &[columns(&["1", "10"]), columns(&["2", "25"])],
            StructuredColumnPolicy::Exact,
        );

        assert_eq!(
            observation.failure,
            Some(StructuredDataObservationFailure::ExpectedRowsMismatch)
        );
        assert!(
            observation
                .diagnostic_message()
                .expect("diagnostic")
                .contains("expected rows")
        );
    }

    #[test]
    fn observation_reports_json_parse_error() {
        let observation = observe_structured_data_schema(
            "output.json",
            r#"{"id":"#,
            &columns(&["id"]),
            &[],
            StructuredColumnPolicy::RequiredOnly,
        );

        assert_eq!(
            observation.failure,
            Some(StructuredDataObservationFailure::ParseError(
                "JSON data artifact is not parse-ready"
            ))
        );
    }
}
