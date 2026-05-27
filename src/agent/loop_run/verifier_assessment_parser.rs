use crate::ollama::xml_fallback::strip_think_tags;

use super::repair_framework_findings::{
    VerifierDiagnosticFrameworkFinding, VerifierDiagnosticFrameworkFindingKind,
};

const VERIFIER_DIAGNOSTIC_MAX_OUTPUT_BYTES: usize = 16_384;
const VERIFIER_DIAGNOSTIC_MAX_SUMMARY_CHARS: usize = 240;
const VERIFIER_DIAGNOSTIC_MAX_REASON_CHARS: usize = 180;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ParsedVerifierRepairAssessment {
    pub(super) failure_kind: super::VerifierDiagnosticFailureKind,
    pub(super) probable_cause_role: Option<super::task_contract::ArtifactRole>,
    pub(super) repair_targets: Vec<ParsedVerifierRepairTarget>,
    pub(super) repair_plan: Vec<ParsedVerifierRepairTarget>,
    pub(super) secondary_targets: Vec<String>,
    pub(super) do_not_edit_tests_without_evidence: bool,
    pub(super) summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ParsedVerifierRepairTarget {
    pub(super) path: String,
    pub(super) confidence: f64,
    pub(super) reason: String,
}

pub(super) fn parse_verifier_repair_assessment_reply(
    reply: &str,
) -> Option<ParsedVerifierRepairAssessment> {
    let value = extract_diagnostic_reply_json_value(reply)?;
    let object = value.as_object()?;
    let failure_kind = object
        .get("failure_kind")
        .or_else(|| object.get("failure_type"))
        .and_then(serde_json::Value::as_str)
        .and_then(verifier_diagnostic_failure_kind_from_str)
        .unwrap_or(super::VerifierDiagnosticFailureKind::Unknown);
    let probable_cause_role = object
        .get("probable_cause_role")
        .or_else(|| object.get("root_cause_role"))
        .or_else(|| object.get("role"))
        .and_then(serde_json::Value::as_str)
        .and_then(artifact_role_from_assessment_str);
    let repair_targets = verifier_assessment_field(object, &["repair_targets", "targets"])
        .map(parse_verifier_repair_targets_value)
        .unwrap_or_default();
    let legacy_target =
        verifier_assessment_field(object, &["repair_target", "target", "target_file", "path"])
            .and_then(parse_verifier_repair_target_value);
    let repair_targets = if repair_targets.is_empty() {
        legacy_target.into_iter().collect::<Vec<_>>()
    } else {
        repair_targets
    };
    let repair_plan = verifier_assessment_field(object, &["repair_plan", "plan", "steps"])
        .map(parse_verifier_repair_targets_value)
        .unwrap_or_default();
    let secondary_targets = verifier_assessment_field(
        object,
        &["secondary_targets", "needed_reads", "related_files"],
    )
    .map(parse_verifier_secondary_targets_value)
    .unwrap_or_default();
    let do_not_edit_tests_without_evidence = object
        .get("do_not_edit_tests_without_evidence")
        .or_else(|| object.get("avoid_test_edits_without_evidence"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let summary = object
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .map(|summary| {
            compact_verifier_failure_text(summary, VERIFIER_DIAGNOSTIC_MAX_SUMMARY_CHARS)
        })
        .filter(|summary| !summary.is_empty());

    Some(ParsedVerifierRepairAssessment {
        failure_kind,
        probable_cause_role,
        repair_targets,
        repair_plan,
        secondary_targets,
        do_not_edit_tests_without_evidence,
        summary,
    })
}

/// Extract the JSON object from a diagnostic LLM reply.
///
/// This is the shared parse boundary for both the legacy assessment parser
/// and the semantic-failure parser in `turn.rs`, so prompt preambles and
/// `<think>` output are stripped consistently before any schema-specific
/// interpretation.
pub(super) fn extract_diagnostic_reply_json_value(reply: &str) -> Option<serde_json::Value> {
    let stripped = strip_think_tags(reply);
    let trimmed = truncate(stripped.trim(), VERIFIER_DIAGNOSTIC_MAX_OUTPUT_BYTES);
    let json_text = if trimmed.starts_with('{') && trimmed.ends_with('}') {
        trimmed.as_str()
    } else {
        let start = trimmed.find('{')?;
        let end = trimmed.rfind('}')?;
        if end <= start {
            return None;
        }
        &trimmed[start..=end]
    };
    serde_json::from_str(json_text).ok()
}

pub(super) fn verifier_failure_type_for_diagnostic_kind(
    kind: super::VerifierDiagnosticFailureKind,
    fallback: super::VerifierFailureType,
) -> super::VerifierFailureType {
    match kind {
        super::VerifierDiagnosticFailureKind::DependencyMissing => {
            super::VerifierFailureType::ImportOrDependency
        }
        // LocalImportContractMismatch maps to ImportOrDependency so local
        // import-source repair selection stays consistent with dependency
        // and symbol-import failures.
        super::VerifierDiagnosticFailureKind::LocalImportContractMismatch => {
            super::VerifierFailureType::ImportOrDependency
        }
        super::VerifierDiagnosticFailureKind::RuntimeError
        | super::VerifierDiagnosticFailureKind::TestBug => super::VerifierFailureType::RuntimeError,
        super::VerifierDiagnosticFailureKind::CompileOrSyntaxError => {
            super::VerifierFailureType::CompileOrSyntax
        }
        super::VerifierDiagnosticFailureKind::AssertionMismatch => {
            super::VerifierFailureType::AssertionFailure
        }
        super::VerifierDiagnosticFailureKind::ConfigOrVerifierError => {
            super::VerifierFailureType::MissingVerifierOrConfig
        }
        super::VerifierDiagnosticFailureKind::Unknown => fallback,
    }
}

pub(super) fn apply_framework_findings_to_parsed_assessment(
    parsed: &mut ParsedVerifierRepairAssessment,
    findings: &[VerifierDiagnosticFrameworkFinding],
) -> bool {
    let Some(finding) = findings
        .iter()
        .find(|finding| finding.role == super::task_contract::ArtifactRole::Test)
    else {
        return false;
    };
    if !framework_finding_can_override_diagnostic_kind(finding.kind, parsed.failure_kind) {
        return false;
    }
    let reason = compact_verifier_failure_text(&finding.summary, 180);
    let target = ParsedVerifierRepairTarget {
        path: finding.path.clone(),
        confidence: 0.95,
        reason: reason.clone(),
    };
    parsed.failure_kind = super::VerifierDiagnosticFailureKind::TestBug;
    parsed.probable_cause_role = Some(super::task_contract::ArtifactRole::Test);
    parsed.do_not_edit_tests_without_evidence = false;
    parsed.summary = Some(reason.clone());
    prepend_unique_parsed_repair_target(&mut parsed.repair_targets, target.clone());
    prepend_unique_parsed_repair_target(&mut parsed.repair_plan, target.clone());
    if !parsed
        .secondary_targets
        .iter()
        .any(|path| path == &finding.path)
    {
        parsed.secondary_targets.insert(0, finding.path.clone());
    }
    true
}

fn framework_finding_can_override_diagnostic_kind(
    finding_kind: VerifierDiagnosticFrameworkFindingKind,
    kind: super::VerifierDiagnosticFailureKind,
) -> bool {
    match kind {
        super::VerifierDiagnosticFailureKind::DependencyMissing
        | super::VerifierDiagnosticFailureKind::LocalImportContractMismatch => {
            matches!(
                finding_kind,
                VerifierDiagnosticFrameworkFindingKind::TestOnlyMissingLocalModuleImport
                    | VerifierDiagnosticFrameworkFindingKind::TestOnlyMissingImportSymbol
                    | VerifierDiagnosticFrameworkFindingKind::RustIntegrationTestCrateImportMismatch
            )
        }
        super::VerifierDiagnosticFailureKind::CompileOrSyntaxError => false,
        super::VerifierDiagnosticFailureKind::ConfigOrVerifierError => matches!(
            finding_kind,
            VerifierDiagnosticFrameworkFindingKind::SetupNameError
                | VerifierDiagnosticFrameworkFindingKind::StatefulClientMissingIsolation
                | VerifierDiagnosticFrameworkFindingKind::UnittestLifecycleMismatch
                | VerifierDiagnosticFrameworkFindingKind::ImportedStateRebindMismatch
                | VerifierDiagnosticFrameworkFindingKind::TestOnlyMissingLocalModuleImport
                | VerifierDiagnosticFrameworkFindingKind::DisconnectedFixtureStateAssertion
                | VerifierDiagnosticFrameworkFindingKind::TestOnlyMissingImportSymbol
                | VerifierDiagnosticFrameworkFindingKind::DisconnectedSetupStateAssignment
        ),
        super::VerifierDiagnosticFailureKind::AssertionMismatch
        | super::VerifierDiagnosticFailureKind::RuntimeError
        | super::VerifierDiagnosticFailureKind::TestBug
        | super::VerifierDiagnosticFailureKind::Unknown => true,
    }
}

fn prepend_unique_parsed_repair_target(
    targets: &mut Vec<ParsedVerifierRepairTarget>,
    target: ParsedVerifierRepairTarget,
) {
    targets.retain(|existing| existing.path != target.path);
    targets.insert(0, target);
}

fn verifier_assessment_field<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<&'a serde_json::Value> {
    keys.iter().find_map(|key| object.get(*key))
}

fn parse_verifier_repair_targets_value(
    value: &serde_json::Value,
) -> Vec<ParsedVerifierRepairTarget> {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .filter_map(parse_verifier_repair_target_value)
            .take(3)
            .collect(),
        _ => parse_verifier_repair_target_value(value)
            .into_iter()
            .collect(),
    }
}

fn parse_verifier_secondary_targets_value(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .filter_map(parse_verifier_repair_target_value)
            .map(|target| target.path)
            .take(3)
            .collect(),
        _ => parse_verifier_repair_target_value(value)
            .map(|target| vec![target.path])
            .unwrap_or_default(),
    }
}

fn parse_verifier_repair_target_value(
    value: &serde_json::Value,
) -> Option<ParsedVerifierRepairTarget> {
    if let Some(path) = value.as_str() {
        let path = path.trim();
        if path.is_empty() || path == "null" || path == "unknown" {
            return None;
        }
        return Some(ParsedVerifierRepairTarget {
            path: path.to_string(),
            confidence: 0.5,
            reason: "diagnostic LLM selected this repair target".to_string(),
        });
    }
    let object = value.as_object()?;
    let path = verifier_assessment_field(
        object,
        &[
            "path",
            "target",
            "file",
            "filename",
            "target_file",
            "target_path",
        ],
    )
    .and_then(serde_json::Value::as_str)?
    .trim();
    if path.is_empty() || path == "null" || path == "unknown" {
        return None;
    }
    let confidence = object
        .get("confidence")
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str().and_then(|text| text.parse::<f64>().ok()))
        })
        .filter(|value| value.is_finite())
        .unwrap_or(0.5)
        .clamp(0.0, 1.0);
    let reason = object
        .get("reason")
        .or_else(|| object.get("intent"))
        .or_else(|| object.get("summary"))
        .or_else(|| object.get("cause"))
        .and_then(serde_json::Value::as_str)
        .map(|reason| compact_verifier_failure_text(reason, VERIFIER_DIAGNOSTIC_MAX_REASON_CHARS))
        .filter(|reason| !reason.is_empty())
        .unwrap_or_else(|| "diagnostic LLM selected this repair target".to_string());
    Some(ParsedVerifierRepairTarget {
        path: path.to_string(),
        confidence,
        reason,
    })
}

fn verifier_diagnostic_failure_kind_from_str(
    value: &str,
) -> Option<super::VerifierDiagnosticFailureKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "dependency_missing" | "import_or_dependency" | "dependency" | "missing_dependency" => {
            Some(super::VerifierDiagnosticFailureKind::DependencyMissing)
        }
        "local_import_contract_mismatch" | "local_symbol_import_error" | "contract_mismatch" => {
            Some(super::VerifierDiagnosticFailureKind::LocalImportContractMismatch)
        }
        "compile_or_syntax_error" | "compile_or_syntax" | "compile" | "syntax" => {
            Some(super::VerifierDiagnosticFailureKind::CompileOrSyntaxError)
        }
        "assertion_mismatch" | "assertion_failure" | "assertion" | "test_assertion" => {
            Some(super::VerifierDiagnosticFailureKind::AssertionMismatch)
        }
        "runtime_error" | "runtime" => Some(super::VerifierDiagnosticFailureKind::RuntimeError),
        "test_bug" | "bad_test" => Some(super::VerifierDiagnosticFailureKind::TestBug),
        "config_or_verifier_error"
        | "missing_verifier_or_config"
        | "missing_verifier"
        | "config" => Some(super::VerifierDiagnosticFailureKind::ConfigOrVerifierError),
        "unknown" => Some(super::VerifierDiagnosticFailureKind::Unknown),
        _ => None,
    }
}

fn artifact_role_from_assessment_str(value: &str) -> Option<super::task_contract::ArtifactRole> {
    match value.trim().to_ascii_lowercase().as_str() {
        "implementation" | "impl" | "code" | "source" => {
            Some(super::task_contract::ArtifactRole::Implementation)
        }
        "test" | "tests" => Some(super::task_contract::ArtifactRole::Test),
        "usage_docs" | "docs" | "documentation" | "readme" => {
            Some(super::task_contract::ArtifactRole::UsageDocs)
        }
        "setup" | "config" | "dependency" | "dependencies" => {
            Some(super::task_contract::ArtifactRole::Setup)
        }
        "unknown" => None,
        _ => None,
    }
}

fn compact_verifier_failure_text(input: &str, max_chars: usize) -> String {
    let masked = crate::session::feedback::mask_secrets(input);
    let collapsed = masked.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&collapsed, max_chars)
}

fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((byte_idx, _)) => {
            let mut out = String::with_capacity(byte_idx + 3);
            out.push_str(&s[..byte_idx]);
            out.push_str("...");
            out
        }
        None => s.to_string(),
    }
}
