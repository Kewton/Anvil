use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const RESULTS_RELATIVE_PATH: &str = "output/results.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultsDocument {
    pub reconciliation: Reconciliation,
    pub values: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reconciliation {
    pub input_rows: u64,
    pub used_rows: u64,
    pub excluded: Vec<ExcludedRows>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExcludedRows {
    pub reason: String,
    pub rows: u64,
}

#[derive(Debug, Error)]
pub enum ResultsSchemaError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid results.json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("results.json missing required key `{0}`")]
    MissingRequiredKey(&'static str),
    #[error("invalid results.json: {0}")]
    Validation(String),
}

impl ResultsDocument {
    pub fn from_json(json: &str) -> Result<Self, ResultsSchemaError> {
        let value: Value = serde_json::from_str(json)?;
        require_keys(&value)?;
        let document: Self = serde_json::from_value(value)?;
        document.validate()?;
        Ok(document)
    }

    pub fn validate(&self) -> Result<(), ResultsSchemaError> {
        if let Some(key) = self.values.keys().find(|key| key.trim().is_empty()) {
            return Err(ResultsSchemaError::Validation(format!(
                "claim key must not be empty: {key:?}"
            )));
        }
        if let Some((key, value)) = self.values.iter().find(|(_, value)| !value.is_finite()) {
            return Err(ResultsSchemaError::Validation(format!(
                "claim value `{key}` must be finite, got {value}"
            )));
        }
        Ok(())
    }
}

pub fn load(root: &Path) -> Result<ResultsDocument, ResultsSchemaError> {
    let path = root.join(RESULTS_RELATIVE_PATH);
    let json = std::fs::read_to_string(&path).map_err(|source| ResultsSchemaError::Read {
        path: path.clone(),
        source,
    })?;
    ResultsDocument::from_json(&json)
}

fn require_keys(value: &Value) -> Result<(), ResultsSchemaError> {
    let root = value
        .as_object()
        .ok_or_else(|| ResultsSchemaError::Validation("root must be an object".to_string()))?;
    let reconciliation = root
        .get("reconciliation")
        .ok_or(ResultsSchemaError::MissingRequiredKey("reconciliation"))?
        .as_object()
        .ok_or_else(|| {
            ResultsSchemaError::Validation("`reconciliation` must be an object".to_string())
        })?;
    for key in ["input_rows", "used_rows", "excluded"] {
        if !reconciliation.contains_key(key) {
            return Err(ResultsSchemaError::MissingRequiredKey(match key {
                "input_rows" => "reconciliation.input_rows",
                "used_rows" => "reconciliation.used_rows",
                _ => "reconciliation.excluded",
            }));
        }
    }
    if !root.contains_key("values") {
        return Err(ResultsSchemaError::MissingRequiredKey("values"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_RESULTS: &str = r#"{
        "reconciliation": {
            "input_rows": 5,
            "used_rows": 3,
            "excluded": [{"reason": "missing amount", "rows": 2}]
        },
        "values": {"total": 12.5, "rate": 40.0}
    }"#;

    #[test]
    fn parses_the_fixed_results_schema() {
        let results = ResultsDocument::from_json(VALID_RESULTS).unwrap();

        assert_eq!(results.reconciliation.input_rows, 5);
        assert_eq!(results.reconciliation.used_rows, 3);
        assert_eq!(results.reconciliation.excluded[0].rows, 2);
        assert_eq!(results.values["total"], 12.5);
    }

    #[test]
    fn missing_required_keys_are_named() {
        for (json, missing) in [
            (r#"{"values": {}}"#, "reconciliation"),
            (
                r#"{"reconciliation":{"used_rows":0,"excluded":[]},"values":{}}"#,
                "input_rows",
            ),
            (
                r#"{"reconciliation":{"input_rows":0,"excluded":[]},"values":{}}"#,
                "used_rows",
            ),
            (
                r#"{"reconciliation":{"input_rows":0,"used_rows":0},"values":{}}"#,
                "excluded",
            ),
            (
                r#"{"reconciliation":{"input_rows":0,"used_rows":0,"excluded":[]}}"#,
                "values",
            ),
        ] {
            let error = ResultsDocument::from_json(json).unwrap_err().to_string();
            assert!(error.contains("missing required key"), "{error}");
            assert!(error.contains(missing), "{error}");
        }
    }

    #[test]
    fn unknown_keys_and_non_row_numbers_are_rejected() {
        let unknown = VALID_RESULTS.replace("\"values\":", "\"unexpected\": true, \"values\":");
        assert!(
            ResultsDocument::from_json(&unknown)
                .unwrap_err()
                .to_string()
                .contains("unknown field")
        );

        let negative = VALID_RESULTS.replace("\"input_rows\": 5", "\"input_rows\": -1");
        assert!(ResultsDocument::from_json(&negative).is_err());
    }

    #[test]
    fn claim_keys_must_be_non_empty() {
        let json = VALID_RESULTS.replace("\"total\"", "\"   \"");
        let error = ResultsDocument::from_json(&json).unwrap_err().to_string();
        assert!(error.contains("claim key must not be empty"), "{error}");
    }

    #[test]
    fn loads_output_results_json_from_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("output")).unwrap();
        std::fs::write(dir.path().join(RESULTS_RELATIVE_PATH), VALID_RESULTS).unwrap();

        let results = load(dir.path()).unwrap();
        assert_eq!(results.values["rate"], 40.0);
    }
}
