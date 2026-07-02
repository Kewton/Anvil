//! v0.4.13 Phase 2: small diagnostic schema for verifier repair.
//!
//! The diagnostic LLM is treated as an untrusted semantic advisor. This module
//! extracts and validates a bounded `RepairBrief`; JSON-adjacent prose is
//! ignored and never becomes control data.

#![allow(dead_code)]

use super::repair_job::sanitize_repair_job_text_with_char_cap;
use super::task_contract::ArtifactRole;

const MAX_BRIEF_TEXT_CHARS: usize = 360;
const MAX_PATH_CHARS: usize = 240;
const MAX_MUST_PRESERVE: usize = 8;
const MIN_REPAIR_CONFIDENCE: f64 = 0.50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairBriefSource {
    DiagnosticLlm,
    LegacyAdapter,
    Controller,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceOfTruth {
    UserRequest,
    BehaviorContract,
    VerifiedPublicInterface,
    ImplementationContract,
    UsageDocs,
    LlmGeneratedTest,
    Ambiguous,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AllowedChangeKind {
    FixImplementationBehavior,
    FixGeneratedTestExpectation,
    ConnectExistingTestSetupToSut,
    FixTestIsolation,
    FixTestImportOrSetup,
    FixDependencyOrConfig,
    FixVerifierCommand,
    InsufficientEvidence,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct RepairBrief {
    pub(super) failure_summary: String,
    pub(super) root_cause: String,
    pub(super) source_of_truth: SourceOfTruth,
    pub(super) repair_target: Option<RepairBriefTarget>,
    pub(super) allowed_change_kind: AllowedChangeKind,
    pub(super) must_preserve: Vec<String>,
    pub(super) concrete_fix_intent: String,
    pub(super) confidence: f64,
    pub(super) source: RepairBriefSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepairBriefTarget {
    pub(super) role: ArtifactRole,
    pub(super) path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct LegacyDiagnosticBriefInput {
    pub(super) failure_kind: String,
    pub(super) probable_cause_role: Option<ArtifactRole>,
    pub(super) repair_target_path: Option<String>,
    pub(super) repair_target_confidence: Option<f64>,
    pub(super) summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RepairBriefParseError {
    JsonMissing,
    JsonMalformed,
    ObjectMissing,
    MissingField(&'static str),
    InvalidRole,
    InvalidAllowedChangeKind,
    InvalidConfidence,
    LowConfidence,
}

impl AllowedChangeKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::FixImplementationBehavior => "fix_implementation_behavior",
            Self::FixGeneratedTestExpectation => "fix_generated_test_expectation",
            Self::ConnectExistingTestSetupToSut => "connect_existing_test_setup_to_sut",
            Self::FixTestIsolation => "fix_test_isolation",
            Self::FixTestImportOrSetup => "fix_test_import_or_setup",
            Self::FixDependencyOrConfig => "fix_dependency_or_config",
            Self::FixVerifierCommand => "fix_verifier_command",
            Self::InsufficientEvidence => "insufficient_evidence",
        }
    }

    pub(super) fn from_str(value: &str) -> Option<Self> {
        match normalize_enum(value).as_str() {
            "fix_implementation_behavior"
            | "implementation_bug_fix"
            | "implementation"
            | "impl" => Some(Self::FixImplementationBehavior),
            "fix_generated_test_expectation" | "test_expectation_alignment" | "test" | "tests" => {
                Some(Self::FixGeneratedTestExpectation)
            }
            "connect_existing_test_setup_to_sut" | "connect_test_setup_to_sut" => {
                Some(Self::ConnectExistingTestSetupToSut)
            }
            "fix_test_isolation" | "test_isolation" => Some(Self::FixTestIsolation),
            "fix_test_import_or_setup" | "fix_test_setup" => Some(Self::FixTestImportOrSetup),
            "fix_dependency_or_config"
            | "dependency_or_config_fix"
            | "dependency"
            | "config"
            | "setup" => Some(Self::FixDependencyOrConfig),
            "fix_verifier_command" | "verifier_command_fix" => Some(Self::FixVerifierCommand),
            "insufficient_evidence"
            | "unknown"
            | "ambiguous"
            | "spec_ambiguous"
            | "spec_ambiguity"
            | "safe_stop" => Some(Self::InsufficientEvidence),
            _ => None,
        }
    }
}

impl RepairBriefSource {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::DiagnosticLlm => "diagnostic_llm",
            Self::LegacyAdapter => "legacy_adapter",
            Self::Controller => "controller",
        }
    }
}

impl SourceOfTruth {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::UserRequest => "user_request",
            Self::BehaviorContract => "behavior_contract",
            Self::VerifiedPublicInterface => "verified_public_interface",
            Self::ImplementationContract => "implementation_contract",
            Self::UsageDocs => "usage_docs",
            Self::LlmGeneratedTest => "llm_generated_test",
            Self::Ambiguous => "ambiguous",
            Self::Unknown => "unknown",
        }
    }

    pub(super) fn from_str(value: &str) -> Self {
        match normalize_enum(value).as_str() {
            "user_request" => Self::UserRequest,
            "behavior_contract" => Self::BehaviorContract,
            "verified_public_interface" => Self::VerifiedPublicInterface,
            "implementation_contract" => Self::ImplementationContract,
            "usage_docs" | "readme" | "docs" => Self::UsageDocs,
            "llm_generated_test" | "generated_test" => Self::LlmGeneratedTest,
            "ambiguous" | "spec_ambiguous" | "spec_ambiguity" => Self::Ambiguous,
            _ => Self::Unknown,
        }
    }
}

impl RepairBriefParseError {
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            Self::JsonMissing => "json_missing",
            Self::JsonMalformed => "json_malformed",
            Self::ObjectMissing => "object_missing",
            Self::MissingField(_) => "missing_field",
            Self::InvalidRole => "invalid_role",
            Self::InvalidAllowedChangeKind => "invalid_allowed_change_kind",
            Self::InvalidConfidence => "invalid_confidence",
            Self::LowConfidence => "low_confidence",
        }
    }
}

impl RepairBrief {
    pub(super) fn from_json_reply(reply: &str) -> Result<Self, RepairBriefParseError> {
        let json = extract_last_json_object(reply).ok_or(RepairBriefParseError::JsonMissing)?;
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|_| RepairBriefParseError::JsonMalformed)?;
        Self::from_value(&value, RepairBriefSource::DiagnosticLlm)
    }

    pub(super) fn from_value(
        value: &serde_json::Value,
        source: RepairBriefSource,
    ) -> Result<Self, RepairBriefParseError> {
        let object = value
            .as_object()
            .ok_or(RepairBriefParseError::ObjectMissing)?;
        let spec_decision = object
            .get("spec_decision")
            .and_then(serde_json::Value::as_object);
        let spec_status = spec_decision
            .and_then(|decision| decision.get("status"))
            .and_then(serde_json::Value::as_str)
            .map(normalize_enum);
        let spec_is_ambiguous = spec_status
            .as_deref()
            .is_some_and(|status| status == "ambiguous" || status == "unresolved");
        let failure_summary = string_field(object, "failure_summary")
            .or_else(|| string_field(object, "summary"))
            .or_else(|| string_field(object, "diagnosis"))
            .unwrap_or_default();
        let root_cause = string_field(object, "root_cause")
            .or_else(|| string_field(object, "diagnosis"))
            .ok_or(RepairBriefParseError::MissingField("root_cause"))?;
        let mut source_of_truth = object
            .get("source_of_truth")
            .or_else(|| spec_decision.and_then(|decision| decision.get("source_of_truth")))
            .and_then(serde_json::Value::as_str)
            .map(SourceOfTruth::from_str)
            .unwrap_or(SourceOfTruth::Unknown);
        if spec_is_ambiguous {
            source_of_truth = SourceOfTruth::Ambiguous;
        }
        let repair_target = parse_target(object)?;
        let allowed_change_kind = if spec_is_ambiguous {
            AllowedChangeKind::InsufficientEvidence
        } else {
            object
                .get("allowed_change_kind")
                .or_else(|| object.get("change_kind"))
                .or_else(|| first_repair_step_field(object, "change_kind"))
                .and_then(serde_json::Value::as_str)
                .and_then(AllowedChangeKind::from_str)
                .ok_or(RepairBriefParseError::InvalidAllowedChangeKind)?
        };
        let must_preserve = object
            .get("must_preserve")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .take(MAX_MUST_PRESERVE)
                    .map(sanitize_brief_text)
                    .filter(|item| !item.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let concrete_fix_intent = string_field(object, "concrete_fix_intent")
            .or_else(|| string_field(object, "concrete_fix"))
            .or_else(|| {
                first_repair_step_field(object, "intent")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let confidence = object
            .get("confidence")
            .and_then(serde_json::Value::as_f64)
            .ok_or(RepairBriefParseError::InvalidConfidence)?;
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err(RepairBriefParseError::InvalidConfidence);
        }
        if confidence < MIN_REPAIR_CONFIDENCE
            && allowed_change_kind != AllowedChangeKind::InsufficientEvidence
        {
            return Err(RepairBriefParseError::LowConfidence);
        }

        Ok(Self {
            failure_summary: sanitize_brief_text(&failure_summary),
            root_cause: sanitize_brief_text(&root_cause),
            source_of_truth,
            repair_target,
            allowed_change_kind,
            must_preserve,
            concrete_fix_intent: sanitize_brief_text(&concrete_fix_intent),
            confidence,
            source,
        })
    }
}

pub(super) fn repair_brief_from_legacy_diagnostic(
    input: LegacyDiagnosticBriefInput,
) -> Result<RepairBrief, RepairBriefParseError> {
    let confidence = input.repair_target_confidence.unwrap_or(0.5);
    if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
        return Err(RepairBriefParseError::InvalidConfidence);
    }
    let target_role = input.probable_cause_role;
    let repair_target = match (target_role, input.repair_target_path) {
        (Some(role), Some(path)) => Some(RepairBriefTarget {
            role,
            path: sanitize_path(&path),
        }),
        _ => None,
    };
    let allowed_change_kind =
        legacy_kind_to_allowed_change_kind_for_role(&input.failure_kind, target_role);
    let root_cause = input
        .summary
        .as_deref()
        .filter(|summary| !summary.trim().is_empty())
        .unwrap_or(input.failure_kind.as_str());

    Ok(RepairBrief {
        failure_summary: sanitize_brief_text(input.summary.as_deref().unwrap_or("")),
        root_cause: sanitize_brief_text(root_cause),
        source_of_truth: SourceOfTruth::Unknown,
        repair_target,
        allowed_change_kind,
        must_preserve: Vec::new(),
        concrete_fix_intent: String::new(),
        confidence,
        source: RepairBriefSource::LegacyAdapter,
    })
}

pub(super) fn extract_last_json_object(reply: &str) -> Option<&str> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut start: Option<usize> = None;
    let mut last: Option<&str> = None;
    for (idx, ch) in reply.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(idx);
                }
                depth = depth.saturating_add(1);
            }
            '}' => {
                if depth == 0 {
                    continue;
                }
                depth -= 1;
                if depth == 0
                    && let Some(start_idx) = start.take()
                {
                    last = Some(&reply[start_idx..idx + ch.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    last
}

fn parse_target(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<RepairBriefTarget>, RepairBriefParseError> {
    let Some(target) = object
        .get("repair_target")
        .or_else(|| first_repair_step_field(object, "target_file"))
        .or_else(|| first_repair_step_field(object, "target_path"))
        .or_else(|| first_repair_step_field(object, "path"))
    else {
        return Ok(None);
    };
    if let Some(path) = target.as_str() {
        let role = first_repair_step_field(object, "role")
            .or_else(|| first_repair_step_field(object, "target_role"))
            .or_else(|| first_repair_step_field(object, "change_kind"))
            .and_then(serde_json::Value::as_str)
            .and_then(artifact_role_from_str)
            .ok_or(RepairBriefParseError::InvalidRole)?;
        return Ok(Some(RepairBriefTarget {
            role,
            path: sanitize_path(path),
        }));
    }
    let Some(target_object) = target.as_object() else {
        return Ok(None);
    };
    let role = target_object
        .get("role")
        .and_then(serde_json::Value::as_str)
        .and_then(artifact_role_from_str)
        .ok_or(RepairBriefParseError::InvalidRole)?;
    let path = target_object
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(sanitize_path)
        .ok_or(RepairBriefParseError::MissingField("repair_target.path"))?;
    Ok(Some(RepairBriefTarget { role, path }))
}

fn first_repair_step_field<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Option<&'a serde_json::Value> {
    object
        .get("repair_steps")
        .or_else(|| object.get("steps"))
        .and_then(serde_json::Value::as_array)
        .and_then(|steps| steps.first())
        .and_then(serde_json::Value::as_object)
        .and_then(|step| step.get(field))
}

fn artifact_role_from_str(value: &str) -> Option<ArtifactRole> {
    let normalized = normalize_enum(value);
    // Issue #920: 2-tier — canonical labels via the `from_label` SSOT, LLM
    // aliases stay local. Behavior-preserving (this parser already accepted
    // `data_output`; the canonical arm now flows through the SSOT).
    if let Some(role) = ArtifactRole::from_label(&normalized) {
        return Some(role);
    }
    match normalized.as_str() {
        "impl" => Some(ArtifactRole::Implementation),
        "tests" => Some(ArtifactRole::Test),
        "docs" | "readme" => Some(ArtifactRole::UsageDocs),
        "dependency" | "config" => Some(ArtifactRole::Setup),
        "data" | "output" => Some(ArtifactRole::DataOutput),
        _ => None,
    }
}

pub(super) fn legacy_kind_to_allowed_change_kind(value: &str) -> AllowedChangeKind {
    legacy_kind_to_allowed_change_kind_for_role(value, None)
}

pub(super) fn legacy_kind_to_allowed_change_kind_for_role(
    value: &str,
    target_role: Option<ArtifactRole>,
) -> AllowedChangeKind {
    let normalized = normalize_enum(value);
    // Issue #920: intentional decision point — this is an EXHAUSTIVE `match` over
    // `Option<ArtifactRole>` (every variant + `None`), so adding a role
    // compile-errors here and forces a deliberate change-kind policy.
    match target_role {
        Some(ArtifactRole::Test) => match normalized.as_str() {
            "compile_or_syntax_error"
            | "runtime_error"
            | "test_bug"
            | "test_setup_bug"
            | "test_import_bug" => AllowedChangeKind::FixTestImportOrSetup,
            "local_import_contract_mismatch" => AllowedChangeKind::ConnectExistingTestSetupToSut,
            "assertion_mismatch" => AllowedChangeKind::FixGeneratedTestExpectation,
            _ => legacy_kind_to_allowed_change_kind_unscoped(&normalized),
        },
        Some(ArtifactRole::Setup) => match normalized.as_str() {
            "missing_verifier" | "dependency_missing" | "config_or_verifier_error" => {
                AllowedChangeKind::FixDependencyOrConfig
            }
            _ => AllowedChangeKind::FixDependencyOrConfig,
        },
        Some(ArtifactRole::UsageDocs) => AllowedChangeKind::InsufficientEvidence,
        Some(ArtifactRole::DataOutput) => AllowedChangeKind::InsufficientEvidence,
        Some(ArtifactRole::Implementation) | None => {
            legacy_kind_to_allowed_change_kind_unscoped(&normalized)
        }
    }
}

fn legacy_kind_to_allowed_change_kind_unscoped(normalized: &str) -> AllowedChangeKind {
    match normalized {
        "dependency_missing" | "config_or_verifier_error" | "missing_verifier" => {
            AllowedChangeKind::FixDependencyOrConfig
        }
        "local_import_contract_mismatch" => AllowedChangeKind::ConnectExistingTestSetupToSut,
        "compile_or_syntax_error" => AllowedChangeKind::FixImplementationBehavior,
        "test_bug" | "test_setup_bug" | "test_import_bug" => {
            AllowedChangeKind::FixTestImportOrSetup
        }
        "runtime_error" => AllowedChangeKind::FixImplementationBehavior,
        "assertion_mismatch" => AllowedChangeKind::FixImplementationBehavior,
        "unknown" => AllowedChangeKind::InsufficientEvidence,
        _ => AllowedChangeKind::FixImplementationBehavior,
    }
}

fn string_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &'static str,
) -> Option<String> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn sanitize_brief_text(input: &str) -> String {
    sanitize_repair_job_text_with_char_cap(input, MAX_BRIEF_TEXT_CHARS)
}

fn sanitize_path(input: &str) -> String {
    sanitize_repair_job_text_with_char_cap(input, MAX_PATH_CHARS)
        .replace('\\', "/")
        .trim()
        .trim_start_matches("./")
        .to_string()
}

fn normalize_enum(value: &str) -> String {
    value.trim().replace(['-', ' '], "_").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Issue #920: 2-tier delegation is behavior-preserving — this parser already
    // accepted `data_output`/`data`/`output`; the canonical arm now flows through
    // the `from_label` SSOT and every alias is retained.
    #[test]
    fn artifact_role_from_str_canonical_via_ssot_and_aliases_preserved() {
        for r in ArtifactRole::all() {
            assert_eq!(artifact_role_from_str(r.label()), Some(r));
        }
        // data_output already worked here — must stay working.
        assert_eq!(
            artifact_role_from_str("data_output"),
            Some(ArtifactRole::DataOutput)
        );
        assert_eq!(
            artifact_role_from_str("data"),
            Some(ArtifactRole::DataOutput)
        );
        assert_eq!(
            artifact_role_from_str("output"),
            Some(ArtifactRole::DataOutput)
        );
        // Aliases preserved.
        assert_eq!(
            artifact_role_from_str("impl"),
            Some(ArtifactRole::Implementation)
        );
        assert_eq!(artifact_role_from_str("tests"), Some(ArtifactRole::Test));
        assert_eq!(
            artifact_role_from_str("docs"),
            Some(ArtifactRole::UsageDocs)
        );
        assert_eq!(
            artifact_role_from_str("readme"),
            Some(ArtifactRole::UsageDocs)
        );
        assert_eq!(
            artifact_role_from_str("dependency"),
            Some(ArtifactRole::Setup)
        );
        assert_eq!(artifact_role_from_str("config"), Some(ArtifactRole::Setup));
        assert_eq!(artifact_role_from_str("nonsense"), None);
    }

    #[test]
    fn repair_brief_extracts_last_json_after_prose() {
        let reply = r#"Thinking...
{"not":"the object"}
{"failure_summary":"fixture disconnected","root_cause":"routes use globals","source_of_truth":"user_request","repair_target":{"role":"implementation","path":"./app\\main.py"},"allowed_change_kind":"connect_existing_test_setup_to_sut","must_preserve":["do not delete tests"],"concrete_fix_intent":"use provider hook","confidence":0.86}"#;

        let brief = RepairBrief::from_json_reply(reply).unwrap();

        assert_eq!(brief.root_cause, "routes use globals");
        assert_eq!(brief.source_of_truth, SourceOfTruth::UserRequest);
        assert_eq!(
            brief.allowed_change_kind,
            AllowedChangeKind::ConnectExistingTestSetupToSut
        );
        let target = brief.repair_target.unwrap();
        assert_eq!(target.role, ArtifactRole::Implementation);
        assert_eq!(target.path, "app/main.py");
        assert_eq!(brief.must_preserve, vec!["do not delete tests"]);
    }

    #[test]
    fn repair_brief_rejects_low_confidence_actionable_brief() {
        let reply = r#"{"root_cause":"unknown","repair_target":{"role":"test","path":"tests/test_main.py"},"allowed_change_kind":"fix_generated_test_expectation","confidence":0.2}"#;

        assert_eq!(
            RepairBrief::from_json_reply(reply),
            Err(RepairBriefParseError::LowConfidence)
        );
    }

    #[test]
    fn repair_brief_allows_low_confidence_insufficient_evidence() {
        let reply = r#"{"root_cause":"unknown","allowed_change_kind":"insufficient_evidence","confidence":0.2}"#;

        let brief = RepairBrief::from_json_reply(reply).unwrap();

        assert_eq!(
            brief.allowed_change_kind,
            AllowedChangeKind::InsufficientEvidence
        );
        assert_eq!(brief.repair_target, None);
    }

    #[test]
    fn repair_brief_accepts_repair_plan_shaped_json() {
        let reply = r#"{
            "diagnosis":"implementation returns a different status code",
            "spec_decision":{"status":"resolved","source_of_truth":"readme"},
            "repair_steps":[{"target_file":"app/main.py","change_kind":"implementation","intent":"align status code with documented API"}],
            "confidence":0.72
        }"#;

        let brief = RepairBrief::from_json_reply(reply).unwrap();

        assert_eq!(
            brief.root_cause,
            "implementation returns a different status code"
        );
        assert_eq!(brief.source_of_truth, SourceOfTruth::UsageDocs);
        assert_eq!(
            brief.allowed_change_kind,
            AllowedChangeKind::FixImplementationBehavior
        );
        let target = brief.repair_target.unwrap();
        assert_eq!(target.role, ArtifactRole::Implementation);
        assert_eq!(target.path, "app/main.py");
        assert_eq!(
            brief.concrete_fix_intent,
            "align status code with documented API"
        );
    }

    #[test]
    fn repair_brief_treats_ambiguous_plan_as_insufficient_evidence() {
        let reply = r#"{
            "diagnosis":"200 and 201 are both plausible",
            "spec_decision":{"status":"ambiguous","source_of_truth":"ambiguous"},
            "repair_steps":[{"target_file":"tests/test_main.py","change_kind":"test","intent":"change expected value"}],
            "confidence":0.82
        }"#;

        let brief = RepairBrief::from_json_reply(reply).unwrap();

        assert_eq!(brief.source_of_truth, SourceOfTruth::Ambiguous);
        assert_eq!(
            brief.allowed_change_kind,
            AllowedChangeKind::InsufficientEvidence
        );
    }

    #[test]
    fn legacy_diagnostic_adapter_builds_bounded_brief() {
        let brief = repair_brief_from_legacy_diagnostic(LegacyDiagnosticBriefInput {
            failure_kind: "test_import_bug".to_string(),
            probable_cause_role: Some(ArtifactRole::Test),
            repair_target_path: Some("./tests\\test_main.py".to_string()),
            repair_target_confidence: Some(0.7),
            summary: Some("generated test imports an internal helper".to_string()),
        })
        .unwrap();

        assert_eq!(brief.source, RepairBriefSource::LegacyAdapter);
        assert_eq!(brief.repair_target.unwrap().path, "tests/test_main.py");
        assert_eq!(
            brief.allowed_change_kind,
            AllowedChangeKind::FixTestImportOrSetup
        );
    }

    #[test]
    fn legacy_diagnostic_adapter_uses_target_role_for_compile_errors() {
        let test_brief = repair_brief_from_legacy_diagnostic(LegacyDiagnosticBriefInput {
            failure_kind: "compile_or_syntax_error".to_string(),
            probable_cause_role: Some(ArtifactRole::Test),
            repair_target_path: Some("tests/lib.rs".to_string()),
            repair_target_confidence: Some(0.95),
            summary: Some("generated test calls the function with the wrong signature".to_string()),
        })
        .unwrap();
        assert_eq!(
            test_brief.allowed_change_kind,
            AllowedChangeKind::FixTestImportOrSetup
        );

        let impl_brief = repair_brief_from_legacy_diagnostic(LegacyDiagnosticBriefInput {
            failure_kind: "compile_or_syntax_error".to_string(),
            probable_cause_role: Some(ArtifactRole::Implementation),
            repair_target_path: Some("src/lib.rs".to_string()),
            repair_target_confidence: Some(0.95),
            summary: Some("implementation has a syntax error".to_string()),
        })
        .unwrap();
        assert_eq!(
            impl_brief.allowed_change_kind,
            AllowedChangeKind::FixImplementationBehavior
        );
    }
}
