//! v0.4.13 Phase 1: bounded verifier-failure packet.
//!
//! `FailurePacket` is the shared input for controller decisions and
//! short-lived diagnostic LLM prompts. It intentionally stores sanitized,
//! bounded summaries rather than raw verifier output or file excerpts.

#![allow(dead_code)]

use std::path::Path;

use sha2::{Digest, Sha256};

use super::repair_job::RepairJob;
use super::repair_job::sanitize_repair_job_text_with_char_cap;
use super::task_contract::ArtifactRole;
use super::task_contract::RecoveryTargetHint;
use super::verifier_failure_artifacts::{
    VERIFIER_OUTPUT_FAILURE_ARTIFACT_REASON, verifier_output_failure_hints,
};

const MAX_COMMAND_CHARS: usize = 360;
const MAX_SIGNATURE_CHARS: usize = 220;
const MAX_OUTPUT_EXCERPT_CHARS: usize = 2000;
const MAX_SHORT_FIELD_CHARS: usize = 240;
const MAX_CASES: usize = 16;
const MAX_OBSERVED_EXPECTED_PAIRS: usize = 16;
const MAX_CANDIDATE_ARTIFACTS: usize = 24;
const MAX_PRIOR_ATTEMPTS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FailurePacket {
    pub(super) verifier_command: String,
    pub(super) failure_signature: String,
    pub(super) failure_kind: String,
    pub(super) diagnostic_code: Option<String>,
    pub(super) timeout_kind: Option<FailurePacketTimeoutKind>,
    pub(super) bounded_output_excerpt: String,
    pub(super) affected_cases: Vec<String>,
    pub(super) observed_expected_pairs: Vec<ObservedExpectedPair>,
    pub(super) candidate_artifacts: Vec<CandidateArtifact>,
    pub(super) prior_attempts: Vec<PriorRepairAttempt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FailurePacketTimeoutKind {
    LongRunningVerifier,
    GeneratedTestHang,
    DependencySetupTimeout,
    EnvironmentStall,
    BuildCommand,
    Unknown,
}

impl FailurePacketTimeoutKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::LongRunningVerifier => "long_running_verifier",
            Self::GeneratedTestHang => "generated_test_hang",
            Self::DependencySetupTimeout => "dependency_setup_timeout",
            Self::EnvironmentStall => "environment_stall_timeout",
            Self::BuildCommand => "build_command_timeout",
            Self::Unknown => "unknown_timeout",
        }
    }

    pub(super) fn repair_hint(self) -> &'static str {
        match self {
            Self::LongRunningVerifier => {
                "inspect the verifier command and reduce it to a bounded project-unit check"
            }
            Self::GeneratedTestHang => {
                "inspect generated tests and implementation loops before retrying the verifier"
            }
            Self::DependencySetupTimeout => {
                "repair dependency setup or safe stop if package installation cannot complete"
            }
            Self::EnvironmentStall => {
                "safe stop if the verifier environment cannot produce bounded evidence"
            }
            Self::BuildCommand => {
                "switch to a bounded test command or repair the build configuration"
            }
            Self::Unknown => "re-diagnose with timeout evidence before choosing a repair target",
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        match label {
            "long_running_verifier" => Some(Self::LongRunningVerifier),
            "generated_test_hang" => Some(Self::GeneratedTestHang),
            "dependency_setup_timeout" => Some(Self::DependencySetupTimeout),
            "environment_stall_timeout" => Some(Self::EnvironmentStall),
            "build_command_timeout" => Some(Self::BuildCommand),
            "unknown_timeout" => Some(Self::Unknown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ObservedExpectedPair {
    pub(super) observed: String,
    pub(super) expected: String,
    pub(super) assertion_shape: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CandidateArtifact {
    pub(super) role: ArtifactRole,
    pub(super) path: String,
    pub(super) reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PriorRepairAttempt {
    pub(super) target_path: String,
    pub(super) target_role: ArtifactRole,
    pub(super) outcome: String,
}

impl FailurePacket {
    pub(super) fn from_repair_job(job: &RepairJob) -> Self {
        Self::from_repair_job_parts(None, job)
    }

    pub(super) fn from_repair_job_with_work_root(work_root: &Path, job: &RepairJob) -> Self {
        Self::from_repair_job_parts(Some(work_root), job)
    }

    fn from_repair_job_parts(work_root: Option<&Path>, job: &RepairJob) -> Self {
        let mut candidate_artifacts = Vec::new();
        if let Some(work_root) = work_root {
            for hint in verifier_output_failure_hints(work_root, &job.output_excerpt) {
                candidate_artifacts.push(CandidateArtifact::from_hint(
                    &hint,
                    VERIFIER_OUTPUT_FAILURE_ARTIFACT_REASON,
                ));
            }
        }
        if let Some(hint) = job.repair_target_hint.as_ref().or(job.target_hint.as_ref()) {
            candidate_artifacts.push(CandidateArtifact::from_hint(
                hint,
                "current verifier repair target candidate",
            ));
        }
        for hint in &job.changed_file_hints {
            candidate_artifacts.push(CandidateArtifact::from_hint(
                hint,
                "changed workspace file is a possible verifier repair target",
            ));
        }
        dedup_candidate_artifacts(&mut candidate_artifacts);

        let prior_attempts = job
            .repair_target_attempt_outcomes
            .iter()
            .take(MAX_PRIOR_ATTEMPTS)
            .map(|outcome| {
                PriorRepairAttempt::new(
                    &outcome.path,
                    outcome.role,
                    &format!("{:?}", outcome.bucket),
                )
            })
            .collect::<Vec<_>>();
        let affected_cases = extract_affected_cases(&job.output_excerpt);
        let observed_expected_pairs = extract_observed_expected_pairs(&job.output_excerpt);

        let mut packet = Self::new(
            &job.command,
            job.error_kind
                .as_deref()
                .unwrap_or_else(|| job.failure_type.as_str()),
            &job.output_excerpt,
            affected_cases,
            observed_expected_pairs,
            candidate_artifacts,
            prior_attempts,
        );
        if packet.timeout_kind.is_none() {
            packet.timeout_kind = job.timeout_kind;
        }
        packet
    }

    pub(super) fn new(
        verifier_command: &str,
        failure_kind: &str,
        verifier_output: &str,
        affected_cases: Vec<String>,
        observed_expected_pairs: Vec<ObservedExpectedPair>,
        candidate_artifacts: Vec<CandidateArtifact>,
        prior_attempts: Vec<PriorRepairAttempt>,
    ) -> Self {
        let bounded_output_excerpt =
            sanitize_repair_job_text_with_char_cap(verifier_output, MAX_OUTPUT_EXCERPT_CHARS);
        let timeout_kind = timeout_kind_from_output(&bounded_output_excerpt);
        let failure_kind = sanitize_short(failure_kind);
        let diagnostic_code =
            structured_diagnostic_code_from_failure_kind(&failure_kind).map(ToString::to_string);
        let affected_cases = affected_cases
            .into_iter()
            .take(MAX_CASES)
            .map(|case| sanitize_short(&case))
            .filter(|case| !case.is_empty())
            .collect::<Vec<_>>();
        let observed_expected_pairs = observed_expected_pairs
            .into_iter()
            .take(MAX_OBSERVED_EXPECTED_PAIRS)
            .map(ObservedExpectedPair::sanitize)
            .collect::<Vec<_>>();
        let candidate_artifacts = candidate_artifacts
            .into_iter()
            .take(MAX_CANDIDATE_ARTIFACTS)
            .map(CandidateArtifact::sanitize)
            .filter(|artifact| !artifact.path.is_empty())
            .collect::<Vec<_>>();
        let prior_attempts = prior_attempts
            .into_iter()
            .take(MAX_PRIOR_ATTEMPTS)
            .map(PriorRepairAttempt::sanitize)
            .filter(|attempt| !attempt.target_path.is_empty())
            .collect::<Vec<_>>();
        let failure_signature = build_failure_signature(
            &failure_kind,
            &bounded_output_excerpt,
            &affected_cases,
            &observed_expected_pairs,
        );

        Self {
            verifier_command: sanitize_repair_job_text_with_char_cap(
                verifier_command,
                MAX_COMMAND_CHARS,
            ),
            failure_signature,
            failure_kind,
            diagnostic_code,
            timeout_kind,
            bounded_output_excerpt,
            affected_cases,
            observed_expected_pairs,
            candidate_artifacts,
            prior_attempts,
        }
    }

    pub(super) fn has_candidate_path(&self, path: &str) -> bool {
        self.candidate_artifacts
            .iter()
            .any(|candidate| candidate.path == path)
    }

    pub(super) fn candidate_role_for_path(&self, path: &str) -> Option<ArtifactRole> {
        self.candidate_artifacts
            .iter()
            .find(|candidate| candidate.path == path)
            .map(|candidate| candidate.role)
    }

    pub(super) fn to_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "verifier_command": &self.verifier_command,
            "failure_signature": &self.failure_signature,
            "failure_kind": &self.failure_kind,
            "diagnostic_code": self.diagnostic_code.as_deref(),
            "timeout_kind": self.timeout_kind.map(FailurePacketTimeoutKind::as_str),
            "bounded_output_excerpt": &self.bounded_output_excerpt,
            "affected_cases": &self.affected_cases,
            "observed_expected_pairs": self.observed_expected_pairs.iter().map(|pair| {
                serde_json::json!({
                    "observed": &pair.observed,
                    "expected": &pair.expected,
                    "assertion_shape": &pair.assertion_shape,
                })
            }).collect::<Vec<_>>(),
            "candidate_artifacts": self.candidate_artifacts.iter().map(|artifact| {
                serde_json::json!({
                    "role": artifact.role.label(),
                    "path": &artifact.path,
                    "reason": &artifact.reason,
                })
            }).collect::<Vec<_>>(),
            "prior_attempts": self.prior_attempts.iter().map(|attempt| {
                serde_json::json!({
                    "target_path": &attempt.target_path,
                    "target_role": attempt.target_role.label(),
                    "outcome": &attempt.outcome,
                })
            }).collect::<Vec<_>>(),
        })
    }
}

fn structured_diagnostic_code_from_failure_kind(failure_kind: &str) -> Option<&'static str> {
    match failure_kind {
        "missing_file" => Some("missing_file"),
        "invalid_manifest" => Some("invalid_manifest"),
        "bad_test" | "test_bug" => Some("bad_test"),
        "wrong_semantics" | "assertion_mismatch" | "assertion_failure" => Some("wrong_semantics"),
        "evidence_missing" | "missing_verifier_or_config" | "config_or_verifier_error" => {
            Some("evidence_missing")
        }
        "schema_mismatch" | "structured_data_schema_missing" => Some("schema_mismatch"),
        _ => None,
    }
}

impl ObservedExpectedPair {
    pub(super) fn new(observed: &str, expected: &str, assertion_shape: &str) -> Self {
        Self {
            observed: sanitize_short(observed),
            expected: sanitize_short(expected),
            assertion_shape: sanitize_short(assertion_shape),
        }
    }

    fn sanitize(self) -> Self {
        Self::new(&self.observed, &self.expected, &self.assertion_shape)
    }
}

impl CandidateArtifact {
    pub(super) fn new(role: ArtifactRole, path: &str, reason: &str) -> Self {
        Self {
            role,
            path: sanitize_path(path),
            reason: sanitize_short(reason),
        }
    }

    fn sanitize(self) -> Self {
        Self::new(self.role, &self.path, &self.reason)
    }

    fn from_hint(hint: &RecoveryTargetHint, reason: &str) -> Self {
        Self::new(hint.role, &hint.path, reason)
    }
}

impl PriorRepairAttempt {
    pub(super) fn new(target_path: &str, target_role: ArtifactRole, outcome: &str) -> Self {
        Self {
            target_path: sanitize_path(target_path),
            target_role,
            outcome: sanitize_short(outcome),
        }
    }

    fn sanitize(self) -> Self {
        Self::new(&self.target_path, self.target_role, &self.outcome)
    }
}

fn sanitize_short(input: &str) -> String {
    sanitize_repair_job_text_with_char_cap(input, MAX_SHORT_FIELD_CHARS)
}

fn sanitize_path(input: &str) -> String {
    sanitize_repair_job_text_with_char_cap(input, MAX_SHORT_FIELD_CHARS)
        .replace('\\', "/")
        .trim()
        .trim_start_matches("./")
        .to_string()
}

fn build_failure_signature(
    failure_kind: &str,
    bounded_output_excerpt: &str,
    affected_cases: &[String],
    pairs: &[ObservedExpectedPair],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(failure_kind.as_bytes());
    hasher.update(b"\n");
    hasher.update(bounded_output_excerpt.as_bytes());
    for case in affected_cases {
        hasher.update(b"\ncase:");
        hasher.update(case.as_bytes());
    }
    for pair in pairs {
        hasher.update(b"\npair:");
        hasher.update(pair.observed.as_bytes());
        hasher.update(b"=>");
        hasher.update(pair.expected.as_bytes());
        hasher.update(b":");
        hasher.update(pair.assertion_shape.as_bytes());
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(MAX_SIGNATURE_CHARS.min(16));
    for byte in digest.iter().take(8) {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

fn dedup_candidate_artifacts(candidates: &mut Vec<CandidateArtifact>) {
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.path.clone()));
}

pub(super) fn timeout_kind_from_output(output: &str) -> Option<FailurePacketTimeoutKind> {
    output
        .split_whitespace()
        .find_map(|token| token.strip_prefix("timeout_kind="))
        .and_then(|label| {
            let label = label.trim_matches(|ch: char| {
                matches!(ch, ',' | ';' | '.' | ')' | ']' | '}' | '"' | '\'')
            });
            FailurePacketTimeoutKind::from_label(label)
        })
}

fn extract_affected_cases(output: &str) -> Vec<String> {
    let mut cases = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("- ") {
            push_case(&mut cases, first_case_token(rest));
        } else if let Some(rest) = trimmed.strip_prefix("FAILED ") {
            push_case(&mut cases, first_case_token(rest));
        } else if let Some(rest) = trimmed.strip_prefix("ERROR ") {
            push_case(
                &mut cases,
                first_case_token(rest.strip_prefix("collecting ").unwrap_or(rest)),
            );
        } else if let Some(rest) = trimmed.strip_prefix("test ") {
            if let Some((name, tail)) = rest.split_once(" ... ")
                && tail.starts_with("FAILED")
            {
                push_case(&mut cases, Some(name));
            }
        } else if trimmed.starts_with("---- ") && trimmed.ends_with(" stdout ----") {
            let case = trimmed
                .trim_start_matches("---- ")
                .trim_end_matches(" stdout ----")
                .trim();
            push_case(&mut cases, Some(case));
        } else if let Some(rest) = trimmed.strip_prefix("thread '")
            && let Some((name, _)) = rest.split_once("' panicked")
        {
            push_case(&mut cases, Some(name));
        } else if trimmed.starts_with('_')
            && trimmed.contains(" ERROR collecting ")
            && let Some((_, path)) = trimmed.split_once(" ERROR collecting ")
        {
            push_case(&mut cases, Some(path.trim_matches('_').trim()));
        }
        if cases.len() >= MAX_CASES {
            break;
        }
    }
    cases
}

fn first_case_token(line: &str) -> Option<&str> {
    line.split_whitespace()
        .next()
        .map(|token| token.trim_end_matches(':'))
        .filter(|token| !token.is_empty())
}

fn push_case(cases: &mut Vec<String>, case: Option<&str>) {
    let Some(case) = case.map(sanitize_short).filter(|case| !case.is_empty()) else {
        return;
    };
    if !cases.iter().any(|existing| existing == &case) {
        cases.push(case);
    }
}

fn extract_observed_expected_pairs(output: &str) -> Vec<ObservedExpectedPair> {
    let mut pairs = Vec::new();
    let mut pending_left: Option<String> = None;
    for line in output.lines() {
        if let Some((observed, expected)) = observed_expected_from_assert_line(line) {
            push_pair(
                &mut pairs,
                ObservedExpectedPair::new(&observed, &expected, "assert_equal"),
            );
        }

        let trimmed = line.trim();
        if let Some(left) = trimmed.strip_prefix("left:") {
            pending_left = Some(sanitize_short(left.trim()));
        } else if let Some(right) = trimmed.strip_prefix("right:")
            && let Some(left) = pending_left.take()
        {
            push_pair(
                &mut pairs,
                ObservedExpectedPair::new(&left, right.trim(), "left_right"),
            );
        }

        if pairs.len() >= MAX_OBSERVED_EXPECTED_PAIRS {
            break;
        }
    }
    pairs
}

fn observed_expected_from_assert_line(line: &str) -> Option<(String, String)> {
    let (_, tail) = line.split_once("assert ")?;
    let (observed, expected) = tail.split_once("==")?;
    let observed = normalize_observed_expected_token(observed)?;
    let expected = normalize_observed_expected_token(expected)?;
    (observed != expected).then_some((observed, expected))
}

fn normalize_observed_expected_token(raw: &str) -> Option<String> {
    let token = raw
        .trim()
        .trim_start_matches('(')
        .chars()
        .take_while(|ch| !ch.is_whitespace() && !matches!(ch, ',' | ')' | ']' | '}' | ':' | ';'))
        .collect::<String>();
    let normalized = token.trim_matches(['\'', '"']).to_string();
    if normalized.is_empty() || normalized.len() > MAX_SHORT_FIELD_CHARS {
        return None;
    }
    let safe = normalized
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'));
    safe.then_some(normalized)
}

fn push_pair(pairs: &mut Vec<ObservedExpectedPair>, pair: ObservedExpectedPair) {
    if !pairs.iter().any(|existing| {
        existing.observed == pair.observed
            && existing.expected == pair.expected
            && existing.assertion_shape == pair.assertion_shape
    }) {
        pairs.push(pair);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_packet_extracts_pytest_cases_and_assert_pairs() {
        let output = r#"
FAILED tests/test_main.py::test_create_item - assert 201 == 200
- tests/test_main.py::test_delete_item
E       AssertionError: assert 204 == 200
"#;

        let cases = extract_affected_cases(output);
        let pairs = extract_observed_expected_pairs(output);

        assert_eq!(cases[0], "tests/test_main.py::test_create_item");
        assert_eq!(cases[1], "tests/test_main.py::test_delete_item");
        assert_eq!(
            pairs,
            vec![
                ObservedExpectedPair::new("201", "200", "assert_equal"),
                ObservedExpectedPair::new("204", "200", "assert_equal"),
            ]
        );
    }

    #[test]
    fn failure_packet_extracts_cargo_cases_and_left_right_pairs() {
        let output = r#"
test counter::tests::increments ... FAILED
---- counter::tests::increments stdout ----
thread 'counter::tests::increments' panicked at src/lib.rs:12:9:
assertion `left == right` failed
  left: 1
 right: 2
"#;

        let cases = extract_affected_cases(output);
        let pairs = extract_observed_expected_pairs(output);

        assert_eq!(cases[0], "counter::tests::increments");
        assert_eq!(
            pairs,
            vec![ObservedExpectedPair::new("1", "2", "left_right")]
        );
    }

    #[test]
    fn failure_packet_keeps_unknown_logs_non_fatal() {
        let packet = FailurePacket::new(
            "custom verifier",
            "unknown",
            "something failed without a standard test runner",
            extract_affected_cases("something failed without a standard test runner"),
            extract_observed_expected_pairs("something failed without a standard test runner"),
            Vec::new(),
            Vec::new(),
        );

        assert!(packet.affected_cases.is_empty());
        assert!(packet.observed_expected_pairs.is_empty());
        assert!(!packet.bounded_output_excerpt.is_empty());
        assert_eq!(packet.timeout_kind, None);
    }

    #[test]
    fn failure_packet_exposes_structured_diagnostic_code() {
        let packet = FailurePacket::new(
            "npm test",
            "invalid_manifest",
            "package.json is not valid JSON",
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );

        assert_eq!(packet.diagnostic_code.as_deref(), Some("invalid_manifest"));
        assert_eq!(
            packet.to_json_value()["diagnostic_code"],
            serde_json::json!("invalid_manifest")
        );
    }

    #[test]
    fn failure_packet_extracts_typed_timeout_kind() {
        let packet = FailurePacket::new(
            "python3 -B -m pytest",
            "timeout",
            "Verifier execution timed out. timeout_kind=generated_test_hang command=python3",
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );

        assert_eq!(
            packet.timeout_kind,
            Some(FailurePacketTimeoutKind::GeneratedTestHang)
        );
        assert_eq!(
            packet.to_json_value()["timeout_kind"],
            serde_json::json!("generated_test_hang")
        );
    }

    #[test]
    fn failure_packet_preserves_repair_job_timeout_kind_when_excerpt_lacks_label() {
        let job = super::super::repair_job::RepairJob {
            output_excerpt: "verifier timed out after bounded execution".to_string(),
            timeout_kind: Some(FailurePacketTimeoutKind::EnvironmentStall),
            ..super::super::repair_job::RepairJob::new_for_test()
        };

        let packet = FailurePacket::from_repair_job(&job);

        assert_eq!(
            packet.timeout_kind,
            Some(FailurePacketTimeoutKind::EnvironmentStall)
        );
    }

    #[test]
    fn failure_packet_includes_verifier_output_named_existing_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let work_root = temp.path();
        std::fs::write(work_root.join("sales.py"), "def summarize(items): pass\n").unwrap();
        std::fs::create_dir_all(work_root.join("tests")).unwrap();
        std::fs::write(
            work_root.join("tests").join("test_sales.py"),
            "def test_total(): pass\n",
        )
        .unwrap();
        let job = super::super::repair_job::RepairJob {
            output_excerpt:
                "FAILED tests/test_sales.py::test_total - AssertionError: expected total"
                    .to_string(),
            target_hint: Some(RecoveryTargetHint {
                role: ArtifactRole::Implementation,
                path: "sales.py".to_string(),
                reason: "changed implementation candidate".to_string(),
            }),
            changed_file_hints: vec![RecoveryTargetHint {
                role: ArtifactRole::Implementation,
                path: "sales.py".to_string(),
                reason: "changed workspace file".to_string(),
            }],
            ..super::super::repair_job::RepairJob::new_for_test()
        };

        let packet = FailurePacket::from_repair_job_with_work_root(work_root, &job);

        assert!(packet.has_candidate_path("tests/test_sales.py"));
        assert_eq!(
            packet.candidate_role_for_path("tests/test_sales.py"),
            Some(ArtifactRole::Test)
        );
        let failure_artifact = packet
            .candidate_artifacts
            .iter()
            .find(|artifact| artifact.path == "tests/test_sales.py")
            .expect("output-named test artifact candidate");
        assert_eq!(
            failure_artifact.reason,
            "verifier output names this failure artifact"
        );
    }

    #[test]
    fn failure_packet_masks_and_bounds_untrusted_text() {
        let packet = FailurePacket::new(
            "pytest --token=abcdef0123456789SECRET",
            "assertion_failure\x07",
            "Authorization: Bearer abcdef0123456789SECRET\nFAILED tests/test_main.py",
            vec!["tests/test_main.py::test_a\x01".to_string()],
            vec![ObservedExpectedPair::new(
                "token=abcdef0123456789SECRET",
                "200",
                "==",
            )],
            vec![CandidateArtifact::new(
                ArtifactRole::Test,
                "./tests\\test_main.py",
                "Authorization: Bearer abcdef0123456789SECRET",
            )],
            vec![PriorRepairAttempt::new(
                "tests/test_main.py",
                ArtifactRole::Test,
                "RejectedMalformed\x7f",
            )],
        );

        assert!(!packet.verifier_command.contains("SECRET"));
        assert!(!packet.bounded_output_excerpt.contains("SECRET"));
        assert!(!packet.affected_cases[0].contains('\x01'));
        assert_eq!(packet.candidate_artifacts[0].path, "tests/test_main.py");
        assert_eq!(packet.failure_signature.len(), 16);
        assert!(packet.has_candidate_path("tests/test_main.py"));
        assert_eq!(
            packet.candidate_role_for_path("tests/test_main.py"),
            Some(ArtifactRole::Test)
        );
    }
}
