use super::completion_evidence::CompletionEvidence;
use super::failure_packet::{CandidateArtifact, FailurePacket};
use super::task_contract::ArtifactRole;
use crate::tools::bash::BashCommandClass;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(super) enum VerifierTaskKind {
    Coding,
    Docs,
    Data,
}

#[allow(dead_code)]
pub(super) trait Verifier {
    fn task_kind(&self) -> VerifierTaskKind;

    fn pass_evidence(
        &self,
        command: &str,
        bound_artifacts_count: Option<usize>,
    ) -> CompletionEvidence;

    fn artifact_evidence(&self, artifact: VerifierArtifact<'_>) -> Option<CompletionEvidence>;

    fn failure_packet(&self, command: &str, failure_kind: &str, output: &str) -> FailurePacket;
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub(super) struct VerifierArtifact<'a> {
    pub(super) path: Option<&'a str>,
    pub(super) excerpt: &'a str,
    pub(super) required_columns: &'a [String],
}

#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub(super) struct CodingVerifier;

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct DocsVerifier;

#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub(super) struct DataVerifier;

impl Verifier for CodingVerifier {
    fn task_kind(&self) -> VerifierTaskKind {
        VerifierTaskKind::Coding
    }

    fn pass_evidence(
        &self,
        command: &str,
        bound_artifacts_count: Option<usize>,
    ) -> CompletionEvidence {
        CompletionEvidence::VerifierExitZero {
            class: BashCommandClass::BuildTest,
            command: command.to_string(),
            bound_test_artifacts_count: bound_artifacts_count,
        }
    }

    fn artifact_evidence(&self, _artifact: VerifierArtifact<'_>) -> Option<CompletionEvidence> {
        None
    }

    fn failure_packet(&self, command: &str, failure_kind: &str, output: &str) -> FailurePacket {
        generic_verifier_failure_packet(command, failure_kind, output)
    }
}

impl Verifier for DocsVerifier {
    fn task_kind(&self) -> VerifierTaskKind {
        VerifierTaskKind::Docs
    }

    fn pass_evidence(
        &self,
        _command: &str,
        _bound_artifacts_count: Option<usize>,
    ) -> CompletionEvidence {
        CompletionEvidence::RequiredSectionsPass { path: None }
    }

    fn artifact_evidence(&self, artifact: VerifierArtifact<'_>) -> Option<CompletionEvidence> {
        self.required_sections_evidence(artifact.path.map(str::to_string), artifact.excerpt)
    }

    fn failure_packet(&self, command: &str, failure_kind: &str, output: &str) -> FailurePacket {
        generic_verifier_failure_packet(command, failure_kind, output)
    }
}

impl DocsVerifier {
    pub(super) fn required_sections_pass(&self, excerpt: &str) -> bool {
        docs_required_sections_pass(excerpt)
    }

    #[allow(dead_code)]
    pub(super) fn required_sections_evidence(
        &self,
        path: Option<String>,
        excerpt: &str,
    ) -> Option<CompletionEvidence> {
        if self.required_sections_pass(excerpt) {
            Some(CompletionEvidence::RequiredSectionsPass { path })
        } else {
            None
        }
    }

    #[allow(dead_code)]
    pub(super) fn required_sections_failure_packet(
        &self,
        path: &str,
        excerpt: &str,
    ) -> FailurePacket {
        FailurePacket::new(
            "docs required sections",
            "required_sections_missing",
            excerpt,
            Vec::new(),
            Vec::new(),
            vec![CandidateArtifact::new(
                ArtifactRole::UsageDocs,
                path,
                "documentation required sections verifier target",
            )],
            Vec::new(),
        )
    }
}

impl Verifier for DataVerifier {
    fn task_kind(&self) -> VerifierTaskKind {
        VerifierTaskKind::Data
    }

    fn pass_evidence(
        &self,
        command: &str,
        bound_artifacts_count: Option<usize>,
    ) -> CompletionEvidence {
        CompletionEvidence::VerifierExitZero {
            class: BashCommandClass::BuildTest,
            command: command.to_string(),
            bound_test_artifacts_count: bound_artifacts_count,
        }
    }

    fn artifact_evidence(&self, artifact: VerifierArtifact<'_>) -> Option<CompletionEvidence> {
        self.structured_data_evidence(
            artifact.path.map(str::to_string),
            artifact.excerpt,
            artifact.required_columns,
        )
    }

    fn failure_packet(&self, command: &str, failure_kind: &str, output: &str) -> FailurePacket {
        generic_verifier_failure_packet(command, failure_kind, output)
    }
}

impl DataVerifier {
    #[allow(dead_code)]
    pub(super) fn structured_data_pass(
        &self,
        path: Option<&str>,
        excerpt: &str,
        columns: &[String],
    ) -> bool {
        structured_data_pass(path, excerpt, columns)
    }

    #[allow(dead_code)]
    pub(super) fn structured_data_evidence(
        &self,
        path: Option<String>,
        excerpt: &str,
        required_columns: &[String],
    ) -> Option<CompletionEvidence> {
        let path_ref = path.as_deref();
        if !self.structured_data_pass(path_ref, excerpt, required_columns) {
            return None;
        }
        let columns = observed_data_columns(path_ref, excerpt, required_columns);
        Some(CompletionEvidence::StructuredDataPass { path, columns })
    }

    #[allow(dead_code)]
    pub(super) fn structured_data_failure_packet(
        &self,
        path: &str,
        excerpt: &str,
    ) -> FailurePacket {
        FailurePacket::new(
            "structured data artifact",
            "structured_data_schema_missing",
            excerpt,
            Vec::new(),
            Vec::new(),
            vec![CandidateArtifact::new(
                ArtifactRole::DataOutput,
                path,
                "structured data verifier target",
            )],
            Vec::new(),
        )
    }
}

#[allow(dead_code)]
fn generic_verifier_failure_packet(
    command: &str,
    failure_kind: &str,
    output: &str,
) -> FailurePacket {
    FailurePacket::new(
        command,
        failure_kind,
        output,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
}

const USAGE_DOCS_SETUP_MARKERS: &[(&str, bool)] = &[
    ("install", true),
    ("setup", false),
    ("configuration", false),
    ("dependency", false),
    ("dependencies", false),
    ("package", false),
    ("requirements", false),
    ("cargo.toml", false),
    ("package.json", false),
    ("pyproject.toml", false),
    ("セットアップ", false),
    ("依存", false),
    ("設定", false),
];
const USAGE_DOCS_RUN_MARKERS: &[(&str, bool)] = &[
    ("run", true),
    ("start", false),
    ("usage", false),
    ("example", false),
    ("build", false),
    ("execute", false),
    ("使用", false),
    ("使い方", false),
    ("実行", false),
    ("例", false),
    ("ビルド", false),
];
const USAGE_DOCS_VERIFY_MARKERS: &[(&str, bool)] = &[
    ("test", false),
    ("verify", false),
    ("check", false),
    ("pytest", false),
    ("cargo test", false),
    ("npm test", false),
    ("テスト", false),
    ("検証", false),
];

fn docs_required_sections_pass(excerpt: &str) -> bool {
    let lower = excerpt.to_ascii_lowercase();
    let hit = |markers: &[(&str, bool)]| -> bool {
        markers
            .iter()
            .any(|(needle, token_boundary)| docs_marker_hit(&lower, needle, *token_boundary))
    };
    let mut categories = 0;
    if hit(USAGE_DOCS_SETUP_MARKERS) {
        categories += 1;
    }
    if hit(USAGE_DOCS_RUN_MARKERS) {
        categories += 1;
    }
    if hit(USAGE_DOCS_VERIFY_MARKERS) {
        categories += 1;
    }
    categories >= 2
}

fn docs_marker_hit(lower: &str, needle: &str, token_boundary: bool) -> bool {
    if token_boundary {
        contains_ascii_token(lower, needle)
    } else {
        lower.contains(needle)
    }
}

fn contains_ascii_token(haystack: &str, needle: &str) -> bool {
    haystack
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|token| token == needle)
}

fn structured_data_pass(path: Option<&str>, excerpt: &str, required_columns: &[String]) -> bool {
    let observed = observed_data_columns(path, excerpt, required_columns);
    if observed.is_empty() {
        return false;
    }
    required_columns
        .iter()
        .all(|column| observed.iter().any(|observed| observed == column))
}

fn observed_data_columns(
    path: Option<&str>,
    excerpt: &str,
    required_columns: &[String],
) -> Vec<String> {
    let normalized_ext = path
        .and_then(|path| std::path::Path::new(path).extension())
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
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

fn sorted_unique(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docs_required_sections_pass_becomes_completion_evidence() {
        let verifier = DocsVerifier;
        let evidence = verifier
            .required_sections_evidence(
                Some("README.md".to_string()),
                "## Setup\nInstall with cargo.\n## Usage\nRun an example.\n",
            )
            .expect("required sections pass");
        assert_eq!(
            evidence,
            CompletionEvidence::RequiredSectionsPass {
                path: Some("README.md".to_string())
            }
        );
    }

    #[test]
    fn docs_verifier_artifact_adapter_emits_required_sections_evidence() {
        let verifier = DocsVerifier;
        let artifact = VerifierArtifact {
            path: Some("README.md"),
            excerpt: "## Setup\nInstall it.\n## Usage\nRun it.\n",
            required_columns: &[],
        };

        assert_eq!(
            verifier.artifact_evidence(artifact),
            Some(CompletionEvidence::RequiredSectionsPass {
                path: Some("README.md".to_string()),
            })
        );
    }

    #[test]
    fn verifier_failure_packet_is_task_kind_independent() {
        let verifier = DocsVerifier;
        let packet = verifier.required_sections_failure_packet("README.md", "only title");
        let json = packet.to_json_value();
        assert!(json.get("task_kind").is_none());
        assert_eq!(json["failure_kind"], "required_sections_missing");
        assert_eq!(json["candidate_artifacts"][0]["role"], "usage_docs");
    }

    #[test]
    fn data_verifier_csv_columns_become_structured_data_evidence() {
        let verifier = DataVerifier;
        let required = vec!["Category".to_string(), "Total".to_string()];
        let evidence = verifier
            .structured_data_evidence(
                Some("output.csv".to_string()),
                "Category,Total\nA,1\n",
                &required,
            )
            .expect("schema pass");

        assert_eq!(
            evidence,
            CompletionEvidence::StructuredDataPass {
                path: Some("output.csv".to_string()),
                columns: required,
            }
        );
    }

    #[test]
    fn data_verifier_rejects_missing_required_column() {
        let verifier = DataVerifier;
        let required = vec!["Category".to_string(), "Total".to_string()];

        assert_eq!(
            verifier.structured_data_evidence(
                Some("output.csv".to_string()),
                "Category,Amount\nA,1\n",
                &required,
            ),
            None
        );
    }

    #[test]
    fn data_verifier_jsonl_columns_become_structured_data_evidence() {
        let verifier = DataVerifier;
        let required = vec!["category".to_string(), "total".to_string()];

        assert_eq!(
            verifier.artifact_evidence(VerifierArtifact {
                path: Some("output.jsonl"),
                excerpt: "{\"category\":\"A\",\"total\":1}\n{\"category\":\"B\",\"total\":2}\n",
                required_columns: &required,
            }),
            Some(CompletionEvidence::StructuredDataPass {
                path: Some("output.jsonl".to_string()),
                columns: required,
            })
        );
    }

    #[test]
    fn data_verifier_failure_packet_targets_data_output() {
        let verifier = DataVerifier;
        let packet = verifier.structured_data_failure_packet("output.csv", "Category\nA\n");
        let json = packet.to_json_value();
        assert_eq!(json["failure_kind"], "structured_data_schema_missing");
        assert_eq!(json["candidate_artifacts"][0]["role"], "data_output");
    }

    #[test]
    fn coding_verifier_pass_evidence_is_build_test() {
        let verifier = CodingVerifier;
        assert_eq!(verifier.task_kind(), VerifierTaskKind::Coding);
        assert_eq!(
            verifier.pass_evidence("cargo test", Some(1)),
            CompletionEvidence::VerifierExitZero {
                class: BashCommandClass::BuildTest,
                command: "cargo test".to_string(),
                bound_test_artifacts_count: Some(1),
            }
        );
    }
}
