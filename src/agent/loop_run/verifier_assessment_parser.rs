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
    serde_json::from_str(json_text)
        .ok()
        .or_else(|| repair_adjacent_top_level_objects(json_text))
        .or_else(|| repair_failure_clusters_tail_fields(json_text))
        .or_else(|| repair_unclosed_objects_before_array_end(json_text))
        .or_else(|| repair_truncated_failure_clusters_tail(json_text))
}

pub(super) fn parse_semantic_failure_report_from_reply(
    reply: &str,
) -> Option<super::semantic_failure::SemanticFailureReport> {
    let value = extract_diagnostic_reply_json_value(reply)?;
    super::semantic_failure::parse_semantic_failure_report(&value)
        .or_else(|| parse_nested_semantic_failure_report(&value))
        .or_else(|| parse_inline_cluster_semantic_failure_report(&value))
}

fn parse_nested_semantic_failure_report(
    value: &serde_json::Value,
) -> Option<super::semantic_failure::SemanticFailureReport> {
    let object = value.as_object()?;
    let nested = object
        .get("SemanticFailureReport")
        .or_else(|| object.get("semantic_failure_report"))?;
    let mut nested = nested.clone();
    let nested_object = nested.as_object_mut()?;
    if !nested_object.contains_key("failure_kind") {
        let failure_kind = object
            .get("failure_kind")
            .or_else(|| object.get("failure_type"))?;
        nested_object.insert("failure_kind".to_string(), failure_kind.clone());
    }
    if !nested_object.contains_key("preferred_repair_role")
        && let Some(role) = object
            .get("probable_cause_role")
            .or_else(|| object.get("root_cause_role"))
            .or_else(|| object.get("role"))
    {
        nested_object.insert("preferred_repair_role".to_string(), role.clone());
    }
    if !nested_object.contains_key("confidence")
        && let Some(confidence) = object.get("confidence")
    {
        nested_object.insert("confidence".to_string(), confidence.clone());
    }
    super::semantic_failure::parse_semantic_failure_report(&nested)
}

fn parse_inline_cluster_semantic_failure_report(
    value: &serde_json::Value,
) -> Option<super::semantic_failure::SemanticFailureReport> {
    let object = value.as_object()?;
    let carrier = object
        .get("failure_clusters")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .filter_map(serde_json::Value::as_object)
        .find(|cluster| cluster_contains_report_fields(cluster))?;
    let mut normalized = value.clone();
    let normalized_object = normalized.as_object_mut()?;
    for key in [
        "contract_conflict",
        "preferred_repair_role",
        "repair_hypothesis",
        "confidence",
    ] {
        if !normalized_object.contains_key(key)
            && let Some(field) = carrier.get(key)
        {
            normalized_object.insert(key.to_string(), field.clone());
        }
    }
    if let Some(clusters) = normalized_object
        .get_mut("failure_clusters")
        .and_then(serde_json::Value::as_array_mut)
    {
        clusters.retain(|cluster| {
            cluster
                .as_object()
                .is_some_and(cluster_contains_observation_fields)
        });
    }
    super::semantic_failure::parse_semantic_failure_report(&normalized)
}

fn cluster_contains_report_fields(object: &serde_json::Map<String, serde_json::Value>) -> bool {
    [
        "contract_conflict",
        "preferred_repair_role",
        "repair_hypothesis",
        "confidence",
    ]
    .iter()
    .any(|key| object.contains_key(*key))
}

fn cluster_contains_observation_fields(
    object: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    [
        "observed",
        "expected",
        "input_shape",
        "assertion_shape",
        "affected_cases",
        "involved_artifacts",
        "target_paths",
    ]
    .iter()
    .any(|key| object.contains_key(*key))
}

fn repair_failure_clusters_tail_fields(raw: &str) -> Option<serde_json::Value> {
    let key_start = raw.find("\"failure_clusters\"")?;
    let array_start = raw[key_start..].find('[')? + key_start;
    let bytes = raw.as_bytes();
    let mut in_string = false;
    let mut escape = false;
    let mut bracket_depth = 1usize;
    let mut brace_depth = 0usize;
    let mut index = array_start + 1;

    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escape {
                escape = false;
            } else if byte == b'\\' {
                escape = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }

        match byte {
            b'"' => in_string = true,
            b'[' => bracket_depth = bracket_depth.saturating_add(1),
            b']' => {
                bracket_depth = bracket_depth.saturating_sub(1);
                if bracket_depth == 0 {
                    return None;
                }
            }
            b'{' => brace_depth = brace_depth.saturating_add(1),
            b'}' => brace_depth = brace_depth.saturating_sub(1),
            b',' if bracket_depth == 1
                && brace_depth == 0
                && starts_with_report_field_tail(&raw[index + 1..]) =>
            {
                let mut repaired = String::with_capacity(raw.len() + 1);
                repaired.push_str(&raw[..index]);
                repaired.push(']');
                repaired.push_str(&raw[index..]);
                return serde_json::from_str(&repaired).ok();
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn repair_truncated_failure_clusters_tail(raw: &str) -> Option<serde_json::Value> {
    let key_start = raw.find("\"failure_clusters\"")?;
    let prefix_end = raw[..key_start].rfind(',')?;
    let prefix = raw[..prefix_end].trim_end();
    if !prefix.starts_with('{') {
        return None;
    }
    let mut repaired = String::with_capacity(prefix.len() + 1);
    repaired.push_str(prefix);
    repaired.push('}');
    let value = serde_json::from_str::<serde_json::Value>(&repaired).ok()?;
    legacy_diagnostic_fields_present(&value).then_some(value)
}

fn repair_unclosed_objects_before_array_end(raw: &str) -> Option<serde_json::Value> {
    let mut repaired = String::with_capacity(raw.len() + 8);
    let mut stack = Vec::<u8>::new();
    let mut in_string = false;
    let mut escape = false;
    let mut changed = false;

    for byte in raw.bytes() {
        if in_string {
            repaired.push(byte as char);
            if escape {
                escape = false;
            } else if byte == b'\\' {
                escape = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }

        match byte {
            b'"' => {
                in_string = true;
                repaired.push('"');
            }
            b'{' | b'[' => {
                stack.push(byte);
                repaired.push(byte as char);
            }
            b'}' => {
                if stack.last() == Some(&b'{') {
                    stack.pop();
                }
                repaired.push('}');
            }
            b']' => {
                while stack.last() == Some(&b'{')
                    && stack
                        .iter()
                        .rev()
                        .nth(1)
                        .is_some_and(|parent| *parent == b'[')
                {
                    repaired.push('}');
                    stack.pop();
                    changed = true;
                }
                if stack.last() == Some(&b'[') {
                    stack.pop();
                }
                repaired.push(']');
            }
            _ => repaired.push(byte as char),
        }
    }

    if !changed {
        return None;
    }
    serde_json::from_str(&repaired)
        .ok()
        .or_else(|| repair_truncated_failure_clusters_tail(&repaired))
}

fn legacy_diagnostic_fields_present(value: &serde_json::Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.contains_key("failure_kind")
        && (object.contains_key("repair_targets") || object.contains_key("targets"))
        && object.contains_key("repair_plan")
}

fn repair_adjacent_top_level_objects(raw: &str) -> Option<serde_json::Value> {
    let parts = split_adjacent_top_level_objects(raw)?;
    let mut merged = serde_json::Map::new();
    for part in parts {
        let value = serde_json::from_str::<serde_json::Value>(part).ok()?;
        let object = value.as_object()?;
        for (key, value) in object {
            merged.insert(key.clone(), value.clone());
        }
    }
    Some(serde_json::Value::Object(merged))
}

fn split_adjacent_top_level_objects(raw: &str) -> Option<Vec<&str>> {
    let bytes = raw.as_bytes();
    let mut parts = Vec::new();
    let mut in_string = false;
    let mut escape = false;
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escape {
                escape = false;
            } else if byte == b'\\' {
                escape = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }

        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => depth = depth.saturating_add(1),
            b'}' | b']' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                let left = raw[start..index].trim();
                let right = raw[index + 1..].trim_start();
                if left.starts_with('{') && left.ends_with('}') && right.starts_with('{') {
                    parts.push(left);
                    start = index + 1;
                }
            }
            _ => {}
        }
        index += 1;
    }

    let tail = raw[start..].trim();
    if parts.is_empty() || !tail.starts_with('{') || !tail.ends_with('}') {
        return None;
    }
    parts.push(tail);
    Some(parts)
}

fn starts_with_report_field_tail(value: &str) -> bool {
    let trimmed = value.trim_start();
    [
        "\"contract_conflict\"",
        "\"preferred_repair_role\"",
        "\"repair_hypothesis\"",
        "\"confidence\"",
    ]
    .iter()
    .any(|field| trimmed.starts_with(field))
}

pub(super) fn verifier_failure_type_for_diagnostic_kind(
    kind: super::VerifierDiagnosticFailureKind,
    fallback: super::VerifierFailureType,
) -> super::VerifierFailureType {
    match kind {
        super::VerifierDiagnosticFailureKind::MissingFile => {
            super::VerifierFailureType::MissingVerifierOrConfig
        }
        super::VerifierDiagnosticFailureKind::InvalidManifest => {
            super::VerifierFailureType::MissingVerifierOrConfig
        }
        super::VerifierDiagnosticFailureKind::BadTest => super::VerifierFailureType::RuntimeError,
        super::VerifierDiagnosticFailureKind::WrongSemantics => {
            super::VerifierFailureType::AssertionFailure
        }
        super::VerifierDiagnosticFailureKind::EvidenceMissing => {
            super::VerifierFailureType::MissingVerifierOrConfig
        }
        super::VerifierDiagnosticFailureKind::SchemaMismatch => {
            super::VerifierFailureType::AssertionFailure
        }
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
        super::VerifierDiagnosticFailureKind::MissingFile
        | super::VerifierDiagnosticFailureKind::InvalidManifest
        | super::VerifierDiagnosticFailureKind::EvidenceMissing
        | super::VerifierDiagnosticFailureKind::SchemaMismatch => false,
        super::VerifierDiagnosticFailureKind::BadTest => true,
        super::VerifierDiagnosticFailureKind::WrongSemantics => false,
        super::VerifierDiagnosticFailureKind::DependencyMissing
        | super::VerifierDiagnosticFailureKind::LocalImportContractMismatch => {
            matches!(
                finding_kind,
                VerifierDiagnosticFrameworkFindingKind::TestOnlyMissingLocalModuleImport
                    | VerifierDiagnosticFrameworkFindingKind::TestOnlyMissingImportSymbol
                    | VerifierDiagnosticFrameworkFindingKind::RustIntegrationTestCrateImportMismatch
                    | VerifierDiagnosticFrameworkFindingKind::NodeModuleSyntaxMismatch
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
                | VerifierDiagnosticFrameworkFindingKind::NodeModuleSyntaxMismatch
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
        "missing_file" | "missing_path" | "path_missing" => {
            Some(super::VerifierDiagnosticFailureKind::MissingFile)
        }
        "invalid_manifest" | "manifest_error" | "malformed_manifest" => {
            Some(super::VerifierDiagnosticFailureKind::InvalidManifest)
        }
        "bad_test" => Some(super::VerifierDiagnosticFailureKind::BadTest),
        "wrong_semantics" | "wrong_behavior" | "semantic_mismatch" => {
            Some(super::VerifierDiagnosticFailureKind::WrongSemantics)
        }
        "evidence_missing" | "missing_evidence" => {
            Some(super::VerifierDiagnosticFailureKind::EvidenceMissing)
        }
        "schema_mismatch" | "schema_error" | "invalid_schema" => {
            Some(super::VerifierDiagnosticFailureKind::SchemaMismatch)
        }
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
        "test_bug" => Some(super::VerifierDiagnosticFailureKind::TestBug),
        "config_or_verifier_error"
        | "missing_verifier_or_config"
        | "missing_verifier"
        | "config" => Some(super::VerifierDiagnosticFailureKind::ConfigOrVerifierError),
        "unknown" => Some(super::VerifierDiagnosticFailureKind::Unknown),
        _ => None,
    }
}

fn artifact_role_from_assessment_str(value: &str) -> Option<super::task_contract::ArtifactRole> {
    use super::task_contract::ArtifactRole;
    let normalized = value.trim().to_ascii_lowercase();
    // Issue #920: 2-tier — canonical labels go through the `from_label` SSOT
    // (so `data_output` now round-trips here, previously dropped); LLM-origin
    // aliases stay local to this parser.
    if let Some(role) = ArtifactRole::from_label(&normalized) {
        return Some(role);
    }
    match normalized.as_str() {
        "impl" | "code" | "source" => Some(ArtifactRole::Implementation),
        "tests" => Some(ArtifactRole::Test),
        "docs" | "documentation" | "readme" => Some(ArtifactRole::UsageDocs),
        "config" | "dependency" | "dependencies" => Some(ArtifactRole::Setup),
        // "unknown" and anything else are intentionally unmapped.
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

#[cfg(test)]
mod tests {
    use super::super::task_contract::ArtifactRole;
    use super::*;

    // Issue #920: canonical labels (incl. the previously-dropped `data_output`)
    // round-trip via the `from_label` SSOT, while every pre-existing LLM alias
    // is preserved by the 2-tier `.or_else()` fallback (regression guard).
    #[test]
    fn assessment_role_parser_canonical_round_trips_and_keeps_aliases() {
        // Canonical labels — now flow through from_label (data_output is the fix).
        for r in ArtifactRole::all() {
            assert_eq!(artifact_role_from_assessment_str(r.label()), Some(r));
        }
        assert_eq!(
            artifact_role_from_assessment_str("data_output"),
            Some(ArtifactRole::DataOutput),
            "data_output must now round-trip (was previously dropped)"
        );
        // Aliases preserved.
        assert_eq!(
            artifact_role_from_assessment_str("impl"),
            Some(ArtifactRole::Implementation)
        );
        assert_eq!(
            artifact_role_from_assessment_str("code"),
            Some(ArtifactRole::Implementation)
        );
        assert_eq!(
            artifact_role_from_assessment_str("source"),
            Some(ArtifactRole::Implementation)
        );
        assert_eq!(
            artifact_role_from_assessment_str("tests"),
            Some(ArtifactRole::Test)
        );
        assert_eq!(
            artifact_role_from_assessment_str("documentation"),
            Some(ArtifactRole::UsageDocs)
        );
        assert_eq!(
            artifact_role_from_assessment_str("readme"),
            Some(ArtifactRole::UsageDocs)
        );
        assert_eq!(
            artifact_role_from_assessment_str("dependency"),
            Some(ArtifactRole::Setup)
        );
        // Case-insensitive trim preserved; unknown stays None.
        assert_eq!(
            artifact_role_from_assessment_str("  IMPL "),
            Some(ArtifactRole::Implementation)
        );
        assert_eq!(artifact_role_from_assessment_str("unknown"), None);
        assert_eq!(artifact_role_from_assessment_str("nonsense"), None);
    }

    #[test]
    fn node_module_syntax_framework_finding_overrides_config_diagnostic_to_test_repair() {
        let mut parsed = ParsedVerifierRepairAssessment {
            failure_kind: super::super::VerifierDiagnosticFailureKind::ConfigOrVerifierError,
            probable_cause_role: Some(ArtifactRole::Implementation),
            repair_targets: vec![ParsedVerifierRepairTarget {
                path: "src/index.js".to_string(),
                confidence: 0.8,
                reason: "diagnostic selected implementation".to_string(),
            }],
            repair_plan: Vec::new(),
            secondary_targets: Vec::new(),
            do_not_edit_tests_without_evidence: true,
            summary: None,
        };
        let findings = vec![VerifierDiagnosticFrameworkFinding {
            kind: VerifierDiagnosticFrameworkFindingKind::NodeModuleSyntaxMismatch,
            path: "tests/cli.test.js".to_string(),
            role: ArtifactRole::Test,
            summary: "package.json type conflicts with test source syntax".to_string(),
        }];

        assert!(apply_framework_findings_to_parsed_assessment(
            &mut parsed,
            &findings
        ));

        assert_eq!(
            parsed.failure_kind,
            super::super::VerifierDiagnosticFailureKind::TestBug
        );
        assert_eq!(parsed.probable_cause_role, Some(ArtifactRole::Test));
        assert!(!parsed.do_not_edit_tests_without_evidence);
        assert_eq!(parsed.repair_targets[0].path, "tests/cli.test.js");
        assert_eq!(parsed.repair_targets[1].path, "src/index.js");
    }

    #[test]
    fn semantic_report_parser_recovers_inline_cluster_report_fields() {
        let reply = r#"{
            "failure_kind":"compile_or_syntax_error",
            "probable_cause_role":"test",
            "repair_targets":[
                {"path":"tests/index.test.js","confidence":1.0,"reason":"SyntaxError in test file"}
            ],
            "repair_plan":[
                {"target":"tests/index.test.js","intent":"complete the try/catch statement","confidence":1.0}
            ],
            "summary":"test runner crashed due to invalid JavaScript syntax",
            "failure_clusters":[
                {
                    "observed":"SyntaxError: Missing catch or finally after try",
                    "expected":"Valid JavaScript test file",
                    "input_shape":"try block in generated test",
                    "assertion_shape":"Node module load failure",
                    "affected_cases":["tests/index.test.js"],
                    "involved_artifacts":["test"]
                },
                {
                    "contract_conflict":{
                        "implementation":"",
                        "test":"Incomplete catch block in tests/index.test.js.",
                        "usage_docs":""
                    },
                    "preferred_repair_role":"test",
                    "repair_hypothesis":"The generated test file has an incomplete try/catch statement.",
                    "confidence":1.0
                }
            ]
        }"#;

        let report = parse_semantic_failure_report_from_reply(reply).expect("semantic parse ok");

        assert_eq!(
            report.failure_kind,
            super::super::VerifierDiagnosticFailureKind::CompileOrSyntaxError
        );
        assert_eq!(report.preferred_repair_role, ArtifactRole::Test);
        assert_eq!(report.failure_clusters.len(), 1);
        assert!(report.repair_hypothesis.contains("try/catch"));
        assert!(report.contract_conflict.test.contains("Incomplete catch"));
    }

    #[test]
    fn diagnostic_parser_recovers_report_fields_leaked_out_of_cluster_array() {
        let reply = r#"{
            "failure_kind":"compile_or_syntax_error",
            "probable_cause_role":"test",
            "repair_targets":[
                {"path":"tests/index.test.js","confidence":1.0,"reason":"ReferenceError in test file"}
            ],
            "repair_plan":[
                {"target":"tests/index.test.js","intent":"replace framework globals with node:test imports","confidence":1.0}
            ],
            "summary":"test runner failed because describe is not defined",
            "failure_clusters":[
                {
                    "observed":"ReferenceError: describe is not defined",
                    "expected":"test runner APIs available before assertions",
                    "input_shape":"plain node test script",
                    "assertion_shape":"framework global call",
                    "affected_cases":["tests/index.test.js"],
                    "involved_artifacts":["test"]
                },
            "contract_conflict":{
                "implementation":"",
                "test":"Test file uses unavailable framework globals.",
                "usage_docs":""
            },
            "preferred_repair_role":"test",
            "repair_hypothesis":"Use Node's built-in test API or a configured test framework.",
            "confidence":1.0
        }"#;

        let assessment =
            parse_verifier_repair_assessment_reply(reply).expect("legacy assessment parses");
        assert_eq!(
            assessment.failure_kind,
            super::super::VerifierDiagnosticFailureKind::CompileOrSyntaxError
        );
        assert_eq!(assessment.probable_cause_role, Some(ArtifactRole::Test));
        assert_eq!(assessment.repair_targets[0].path, "tests/index.test.js");

        let report = parse_semantic_failure_report_from_reply(reply).expect("semantic parse ok");
        assert_eq!(report.preferred_repair_role, ArtifactRole::Test);
        assert_eq!(report.failure_clusters.len(), 1);
        assert!(report.contract_conflict.test.contains("framework globals"));
    }

    #[test]
    fn diagnostic_parser_merges_adjacent_legacy_and_semantic_objects() {
        let reply = r#"{
            "failure_kind":"invalid_manifest",
            "probable_cause_role":"setup",
            "repair_targets":[
                {"path":"Cargo.toml","confidence":0.9,"reason":"missing test.name"}
            ],
            "repair_plan":[
                {"target":"Cargo.toml","intent":"add name to the test target","confidence":1.0}
            ],
            "summary":"Cargo manifest is missing the test target name."
        },{
            "failure_clusters":[
                {
                    "observed":"test target test.name is required",
                    "expected":"each [[test]] section has a name",
                    "input_shape":"Cargo.toml [[test]] section",
                    "assertion_shape":"manifest parse error",
                    "affected_cases":["cargo test"],
                    "involved_artifacts":["setup"]
                },
                {
                    "contract_conflict":{
                        "implementation":"",
                        "test":"",
                        "usage_docs":""
                    },
                    "preferred_repair_role":"setup",
                    "repair_hypothesis":"Cargo.toml needs a test target name.",
                    "confidence":1.0
                }
            ]
        }"#;

        let assessment =
            parse_verifier_repair_assessment_reply(reply).expect("legacy assessment parses");
        assert_eq!(
            assessment.failure_kind,
            super::super::VerifierDiagnosticFailureKind::InvalidManifest
        );
        assert_eq!(assessment.probable_cause_role, Some(ArtifactRole::Setup));
        assert_eq!(assessment.repair_targets[0].path, "Cargo.toml");

        let report = parse_semantic_failure_report_from_reply(reply).expect("semantic parse ok");
        assert_eq!(report.preferred_repair_role, ArtifactRole::Setup);
        assert_eq!(report.failure_clusters.len(), 1);
        assert!(report.repair_hypothesis.contains("Cargo.toml"));
    }

    #[test]
    fn diagnostic_parser_recovers_legacy_fields_from_truncated_failure_clusters() {
        let reply = r#"{
            "failure_kind":"compile_or_syntax_error",
            "probable_cause_role":"test",
            "repair_targets":[
                {"path":"tests/palindrome.rs","confidence":1.0,"reason":"invalid Rust comment syntax"}
            ],
            "repair_plan":[
                {"target":"tests/palindrome.rs","intent":"replace markdown heading with Rust comment","confidence":1.0}
            ],
            "secondary_targets":[],
            "do_not_edit_tests_without_evidence":true,
            "summary":"Fix syntax error in tests/palindrome.rs.",
            "failure_clusters":[
                {
                    "observed":"expected one of `!` or `[`, found `Palindrome`",
                    "expected":"valid Rust source",
                    "input_shape":"Rust test file",
                    "assertion_shape":"compile succeeds",
                    "affected_cases":["tests/palindrome.rs"],
                    "involved_artifacts":["test"]
                },
                {
                    "observed":"repeated cluster starts but response is truncated""#;

        let assessment =
            parse_verifier_repair_assessment_reply(reply).expect("legacy assessment parses");

        assert_eq!(
            assessment.failure_kind,
            super::super::VerifierDiagnosticFailureKind::CompileOrSyntaxError
        );
        assert_eq!(assessment.probable_cause_role, Some(ArtifactRole::Test));
        assert_eq!(assessment.repair_targets[0].path, "tests/palindrome.rs");
    }

    #[test]
    fn diagnostic_parser_recovers_missing_target_object_close_before_truncated_clusters() {
        let reply = r#"{
            "failure_kind":"invalid_manifest",
            "probable_cause_role":"setup",
            "repair_targets":[
                {"path":"Cargo.toml","confidence":1.0,"reason":"test target name is required"],
            "repair_plan":[
                {"target":"Cargo.toml","intent":"add a name to the test target","confidence":1.0}
            ],
            "secondary_targets":[],
            "do_not_edit_tests_without_evidence":true,
            "summary":"Fix Cargo.toml by adding a valid test target name.",
            "failure_clusters":[
                {
                    "observed":"test target test.name is required",
                    "expected":"[[test]] has a name",
                    "input_shape":"Cargo.toml [[test]]",
                    "assertion_shape":"manifest parse",
                    "affected_cases":["cargo test"],
                    "involved_artifacts":["setup"]
                },
                {
                    "observed":"repeated cluster starts but response is truncated""#;

        let assessment =
            parse_verifier_repair_assessment_reply(reply).expect("legacy assessment parses");

        assert_eq!(
            assessment.failure_kind,
            super::super::VerifierDiagnosticFailureKind::InvalidManifest
        );
        assert_eq!(assessment.probable_cause_role, Some(ArtifactRole::Setup));
        assert_eq!(assessment.repair_targets[0].path, "Cargo.toml");
    }
}
