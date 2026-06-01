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

    fn failure_packet(&self, command: &str, failure_kind: &str, output: &str) -> FailurePacket;
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

    fn failure_packet(&self, command: &str, failure_kind: &str, output: &str) -> FailurePacket {
        generic_verifier_failure_packet(command, failure_kind, output)
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
    fn verifier_failure_packet_is_task_kind_independent() {
        let verifier = DocsVerifier;
        let packet = verifier.required_sections_failure_packet("README.md", "only title");
        let json = packet.to_json_value();
        assert!(json.get("task_kind").is_none());
        assert_eq!(json["failure_kind"], "required_sections_missing");
        assert_eq!(json["candidate_artifacts"][0]["role"], "usage_docs");
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
