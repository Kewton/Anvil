use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeAcceptanceReport {
    pub passed: bool,
    pub inconclusive: bool,
    pub missing_capabilities: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub missing_obligations: Vec<String>,
    pub weak_evidence: Vec<String>,
    pub inconclusive_reasons: Vec<String>,
    pub artifact_obligations: Vec<ArtifactObligationEvidence>,
    pub capability_evidence_bindings: Vec<CapabilityEvidenceBinding>,
    pub obligation_repair_targets: Vec<ObligationRepairTarget>,
    pub browser_readiness_status: String,
    pub browser_readiness_evidence_path: String,
    pub interaction_evidence_status: String,
    pub interaction_evidence_path: String,
    pub primary_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactObligationEvidence {
    pub path: String,
    pub role: String,
    pub evidence: Vec<String>,
    pub satisfies_implementation: bool,
    pub required_capabilities_supported: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilityEvidenceBinding {
    pub capability: String,
    pub required_evidence: Vec<String>,
    pub satisfied_evidence: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub artifact_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObligationRepairTarget {
    pub obligation: String,
    pub target_role: String,
    pub target_path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BrowserInteractionEvidence {
    pub browser_readiness_status: String,
    pub browser_readiness_evidence_path: String,
    pub interaction_evidence_status: String,
    pub interaction_evidence_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvidenceKind {
    ImplementationArtifact,
    TestArtifact,
    BoundVerifyCommand,
    NonZeroTestOrAssertionEvidence,
    BuildCommandOrDependencyBoundary,
    InteractiveUiSourceEvidence,
    NonStaticScreenEvidence,
    VisibleInteractiveSurfaceEvidence,
    UserInputHandlerEvidence,
    StatefulUpdateEvidence,
    ChallengeOrAdversaryEvidence,
    ScoreOrProgressionEvidence,
    FailureOrCollisionEvidence,
    RestartOrRecoverableStateEvidence,
    NextJsRouteEvidence,
    RequestedContent,
}

impl EvidenceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::ImplementationArtifact => "implementation_artifact",
            Self::TestArtifact => "test_artifact",
            Self::BoundVerifyCommand => "bound_verify_command",
            Self::NonZeroTestOrAssertionEvidence => "non_zero_test_or_assertion_evidence",
            Self::BuildCommandOrDependencyBoundary => {
                "build_command_or_dependency_missing_boundary"
            }
            Self::InteractiveUiSourceEvidence => "interactive_ui_source_evidence",
            Self::NonStaticScreenEvidence => "non_static_screen_evidence",
            Self::VisibleInteractiveSurfaceEvidence => "visible_interactive_surface_evidence",
            Self::UserInputHandlerEvidence => "user_input_handler_evidence",
            Self::StatefulUpdateEvidence => "stateful_update_evidence",
            Self::ChallengeOrAdversaryEvidence => "challenge_or_adversary_evidence",
            Self::ScoreOrProgressionEvidence => "score_or_progression_evidence",
            Self::FailureOrCollisionEvidence => "failure_or_collision_evidence",
            Self::RestartOrRecoverableStateEvidence => "restart_or_recoverable_state_evidence",
            Self::NextJsRouteEvidence => "nextjs_route_evidence",
            Self::RequestedContent => "requested_content_evidence",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtifactRoleLite {
    Setup,
    Scaffold,
    Style,
    Implementation,
    Verification,
    AcceptanceEvidence,
}

impl ArtifactRoleLite {
    fn as_str(self) -> &'static str {
        match self {
            Self::Setup => "setup",
            Self::Scaffold => "scaffold",
            Self::Style => "style",
            Self::Implementation => "implementation",
            Self::Verification => "verification",
            Self::AcceptanceEvidence => "acceptance_evidence",
        }
    }

    fn satisfies_implementation(self) -> bool {
        self == Self::Implementation
    }
}

#[derive(Debug, Default)]
struct WorkspaceEvidence {
    source_files: Vec<SourceFile>,
    test_files: Vec<SourceFile>,
    package_json: Option<String>,
    cargo_toml: bool,
    readme: bool,
}

#[derive(Debug, Clone)]
struct SourceFile {
    rel: String,
    content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserEvidenceKind {
    BrowserReadiness,
    Interaction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BrowserEvidenceStatus {
    Passed,
    Failed(String),
    Unavailable(String),
}

impl BrowserEvidenceStatus {
    fn as_status(&self) -> String {
        match self {
            Self::Passed => "passed".to_string(),
            Self::Failed(reason) => format!("failed:{reason}"),
            Self::Unavailable(reason) => format!("unavailable:{reason}"),
        }
    }
}

pub fn browser_interaction_evidence_for_dirs(
    root: &Path,
    extra_dirs: &[PathBuf],
) -> BrowserInteractionEvidence {
    let browser = read_browser_evidence(
        root,
        extra_dirs,
        &[
            "browser-readiness.json",
            "browser.json",
            "browser-readiness-evidence.json",
        ],
        BrowserEvidenceKind::BrowserReadiness,
        "browser_readiness_evidence_missing",
    );
    let interaction = read_browser_evidence(
        root,
        extra_dirs,
        &[
            "interaction-evidence.json",
            "interaction.json",
            "browser-interaction.json",
        ],
        BrowserEvidenceKind::Interaction,
        "interaction_evidence_missing",
    );
    BrowserInteractionEvidence {
        browser_readiness_status: browser.0.as_status(),
        browser_readiness_evidence_path: browser.1,
        interaction_evidence_status: interaction.0.as_status(),
        interaction_evidence_path: interaction.1,
    }
}

fn read_browser_evidence(
    root: &Path,
    extra_dirs: &[PathBuf],
    names: &[&str],
    kind: BrowserEvidenceKind,
    missing_reason: &'static str,
) -> (BrowserEvidenceStatus, String) {
    for path in browser_evidence_candidate_paths(root, extra_dirs, names) {
        if !path.is_file() {
            continue;
        }
        let display = path.display().to_string();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (
                BrowserEvidenceStatus::Failed("evidence_unreadable".to_string()),
                display,
            );
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return (
                BrowserEvidenceStatus::Failed("evidence_invalid_json".to_string()),
                display,
            );
        };
        if !value.is_object() {
            return (
                BrowserEvidenceStatus::Failed("evidence_invalid_json".to_string()),
                display,
            );
        }
        return (classify_browser_evidence_json(kind, &value), display);
    }
    (
        BrowserEvidenceStatus::Unavailable(missing_reason.to_string()),
        String::new(),
    )
}

fn browser_evidence_candidate_paths(
    root: &Path,
    extra_dirs: &[PathBuf],
    names: &[&str],
) -> Vec<PathBuf> {
    let mut dirs = extra_dirs.to_vec();
    dirs.push(root.join(".anvil"));
    dirs.push(root.to_path_buf());
    let mut out = Vec::new();
    for dir in dirs {
        for name in names {
            let path = dir.join(name);
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

fn classify_browser_evidence_json(
    kind: BrowserEvidenceKind,
    value: &Value,
) -> BrowserEvidenceStatus {
    let details = value
        .get("browser_details")
        .or_else(|| value.get("details"))
        .filter(|value| value.is_object());
    if let Some(status) =
        numeric_field_deep(value, details, &["http_status", "status", "status_code"])
        && status >= 400
    {
        return BrowserEvidenceStatus::Failed(format!("http_{status}"));
    }
    if let Some(success) = bool_field_deep(
        value,
        details,
        &["ok", "success", "browser_success", "interaction_success"],
    ) && !success
    {
        return BrowserEvidenceStatus::Failed(browser_evidence_failure_reason(value, details));
    }
    if let Some(reason) = explicit_browser_evidence_failure(kind, value, details) {
        return BrowserEvidenceStatus::Failed(reason);
    }
    if let Some(status) = text_field_deep(value, details, &["status"]) {
        if matches!(
            status.as_str(),
            "not_enabled" | "adapter_not_implemented" | "unavailable" | "skipped"
        ) {
            return BrowserEvidenceStatus::Unavailable(status);
        }
        if matches!(status.as_str(), "failed" | "fail" | "error") {
            return BrowserEvidenceStatus::Failed(browser_evidence_failure_reason(value, details));
        }
    }
    if let Some(kind_value) = text_field_deep(
        value,
        details,
        &["browser_failure_kind", "failure_kind", "error_kind"],
    ) && !kind_value.is_empty()
    {
        return BrowserEvidenceStatus::Failed(kind_value);
    }
    if browser_evidence_has_required_detail(kind, value, details) {
        return BrowserEvidenceStatus::Passed;
    }
    let success_like = bool_field_deep(
        value,
        details,
        &["ok", "success", "browser_success", "interaction_success"],
    ) == Some(true)
        || text_field_deep(value, details, &["status"])
            .is_some_and(|status| matches!(status.as_str(), "ok" | "pass" | "passed" | "ready"))
        || numeric_field_deep(value, details, &["http_status", "status", "status_code"])
            .is_some_and(|status| (200..400).contains(&status));
    if success_like {
        return BrowserEvidenceStatus::Unavailable(
            match kind {
                BrowserEvidenceKind::BrowserReadiness => "browser_render_evidence_missing",
                BrowserEvidenceKind::Interaction => "interaction_detail_missing",
            }
            .to_string(),
        );
    }
    BrowserEvidenceStatus::Unavailable("evidence_inconclusive".to_string())
}

fn explicit_browser_evidence_failure(
    kind: BrowserEvidenceKind,
    value: &Value,
    details: Option<&Value>,
) -> Option<String> {
    match kind {
        BrowserEvidenceKind::BrowserReadiness => {
            if bool_field_deep(
                value,
                details,
                &["route_rendered", "rendered", "page_loaded", "dom_ready"],
            ) == Some(false)
            {
                return Some("browser_route_not_rendered".to_string());
            }
        }
        BrowserEvidenceKind::Interaction => {
            if bool_field_deep(value, details, &["canvas_found", "canvas_available"]) == Some(false)
            {
                return Some("canvas_unavailable".to_string());
            }
            if bool_field_deep(
                value,
                details,
                &["interactive_surface", "interaction_surface"],
            ) == Some(false)
            {
                return Some("interactive_surface_missing".to_string());
            }
            if bool_field_deep(
                value,
                details,
                &[
                    "input_event_observed",
                    "keyboard_event_observed",
                    "pointer_event_observed",
                ],
            ) == Some(false)
            {
                return Some("input_event_missing".to_string());
            }
            if bool_field_deep(value, details, &["state_changed", "visible_state_changed"])
                == Some(false)
            {
                return Some("interaction_state_change_missing".to_string());
            }
        }
    }
    None
}

fn browser_evidence_has_required_detail(
    kind: BrowserEvidenceKind,
    value: &Value,
    details: Option<&Value>,
) -> bool {
    match kind {
        BrowserEvidenceKind::BrowserReadiness => {
            bool_field_deep(
                value,
                details,
                &["route_rendered", "rendered", "page_loaded", "dom_ready"],
            ) == Some(true)
        }
        BrowserEvidenceKind::Interaction => {
            bool_field_deep(
                value,
                details,
                &[
                    "interaction_performed",
                    "basic_interaction",
                    "interaction_success",
                    "input_event_observed",
                    "keyboard_event_observed",
                    "pointer_event_observed",
                    "state_changed",
                    "visible_state_changed",
                ],
            ) == Some(true)
        }
    }
}

fn browser_evidence_failure_reason(value: &Value, details: Option<&Value>) -> String {
    if let Some(status) =
        numeric_field_deep(value, details, &["http_status", "status", "status_code"])
        && status >= 400
    {
        return format!("http_{status}");
    }
    text_field_deep(
        value,
        details,
        &[
            "browser_failure_kind",
            "failure_kind",
            "error_kind",
            "status",
        ],
    )
    .unwrap_or_else(|| "browser_check_failed".to_string())
}

fn record_browser_acceptance_evidence(
    evidence: &BrowserInteractionEvidence,
    missing_evidence: &mut Vec<String>,
    inconclusive_reasons: &mut Vec<String>,
) {
    record_browser_status(
        "browser_readiness",
        &evidence.browser_readiness_status,
        missing_evidence,
        inconclusive_reasons,
    );
    record_browser_status(
        "browser_interaction",
        &evidence.interaction_evidence_status,
        missing_evidence,
        inconclusive_reasons,
    );
}

fn record_browser_status(
    label: &str,
    status: &str,
    missing_evidence: &mut Vec<String>,
    inconclusive_reasons: &mut Vec<String>,
) {
    if let Some(reason) = status.strip_prefix("failed:") {
        missing_evidence.push(format!("{label}_failed:{reason}"));
    } else if let Some(reason) = status.strip_prefix("unavailable:") {
        inconclusive_reasons.push(format!("{label}_unavailable:{reason}"));
    }
}

fn bool_field_deep(value: &Value, details: Option<&Value>, keys: &[&str]) -> Option<bool> {
    bool_field(value, keys).or_else(|| details.and_then(|details| bool_field(details, keys)))
}

fn bool_field(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_bool))
}

fn numeric_field_deep(value: &Value, details: Option<&Value>, keys: &[&str]) -> Option<i64> {
    numeric_field(value, keys).or_else(|| details.and_then(|details| numeric_field(details, keys)))
}

fn numeric_field(value: &Value, keys: &[&str]) -> Option<i64> {
    for key in keys {
        let Some(raw) = value.get(*key) else {
            continue;
        };
        if let Some(number) = raw.as_i64() {
            return Some(number);
        }
        if let Some(text) = raw.as_str()
            && let Ok(number) = text.parse::<i64>()
        {
            return Some(number);
        }
    }
    None
}

fn text_field_deep(value: &Value, details: Option<&Value>, keys: &[&str]) -> Option<String> {
    text_field(value, keys).or_else(|| details.and_then(|details| text_field(details, keys)))
}

fn text_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(|text| text.trim().to_ascii_lowercase())
}

pub fn required_evidence_for_capability(capability: &str) -> Vec<String> {
    evidence_kinds_for_capability(capability)
        .into_iter()
        .map(|kind| kind.as_str().to_string())
        .collect()
}

pub fn verify_runtime_acceptance(
    root: &Path,
    required_paths: &[String],
    verify_commands: &[String],
    required_capabilities: &[String],
    required_evidence: &[String],
    required_obligations: &[String],
    deferred_verify_requirements: &[String],
) -> RuntimeAcceptanceReport {
    verify_runtime_acceptance_with_browser_dirs(
        root,
        required_paths,
        verify_commands,
        required_capabilities,
        required_evidence,
        required_obligations,
        deferred_verify_requirements,
        &[],
    )
}

pub fn verify_runtime_acceptance_with_browser_dirs(
    root: &Path,
    required_paths: &[String],
    verify_commands: &[String],
    required_capabilities: &[String],
    required_evidence: &[String],
    required_obligations: &[String],
    deferred_verify_requirements: &[String],
    browser_evidence_dirs: &[PathBuf],
) -> RuntimeAcceptanceReport {
    if required_capabilities.is_empty()
        && required_evidence.is_empty()
        && required_obligations.is_empty()
    {
        return RuntimeAcceptanceReport {
            passed: true,
            primary_reason: "pass".to_string(),
            ..RuntimeAcceptanceReport::default()
        };
    }

    let workspace = collect_workspace_evidence(root);
    let artifact_obligations =
        artifact_obligation_evidence(root, required_paths, required_capabilities);
    let mut required = BTreeSet::new();
    let mut missing_capabilities = Vec::new();
    for capability in required_capabilities {
        let kinds = evidence_kinds_for_capability(capability);
        if kinds.is_empty() {
            missing_capabilities.push(format!("unsupported_required_capability:{capability}"));
        }
        for kind in kinds {
            required.insert(kind.as_str().to_string());
        }
    }
    for evidence in required_evidence {
        let trimmed = evidence.trim();
        if !trimmed.is_empty() {
            required.insert(trimmed.to_string());
        }
    }

    let mut missing_evidence = Vec::new();
    let mut weak_evidence = Vec::new();
    let mut inconclusive_reasons = Vec::new();
    let browser_required = required_capabilities
        .iter()
        .any(|capability| capability.trim() == "browser_interaction");
    let browser_interaction = browser_required
        .then(|| browser_interaction_evidence_for_dirs(root, browser_evidence_dirs));
    if let Some(evidence) = &browser_interaction {
        record_browser_acceptance_evidence(
            evidence,
            &mut missing_evidence,
            &mut inconclusive_reasons,
        );
    }
    for evidence in &required {
        match evidence.as_str() {
            "implementation_artifact" => {
                if !has_implementation_artifact(root, required_paths, &workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "test_artifact" => {
                if !has_test_artifact(&workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "bound_verify_command" => {
                if !has_bound_verify_command(verify_commands, &workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "non_zero_test_or_assertion_evidence" => {
                if !has_assertion_or_test_evidence(&workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "build_command_or_dependency_missing_boundary" => {
                if !has_build_command_or_dependency_boundary(
                    verify_commands,
                    deferred_verify_requirements,
                    &workspace,
                ) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "interactive_ui_source_evidence" => {
                if !has_interactive_ui_source(&workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "non_static_screen_evidence" => {
                if !has_non_static_screen_evidence(&workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "visible_interactive_surface_evidence" => {
                if !has_visible_interactive_surface_evidence(&workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "user_input_handler_evidence" => {
                if !has_user_input_handler_evidence(&workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "stateful_update_evidence" => {
                if !has_stateful_update_evidence(&workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "challenge_or_adversary_evidence" => {
                if !has_challenge_or_adversary_evidence(&workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "score_or_progression_evidence" => {
                if !has_score_or_progression_evidence(&workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "failure_or_collision_evidence" => {
                if !has_failure_or_collision_evidence(&workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "restart_or_recoverable_state_evidence" => {
                if !has_restart_or_recoverable_state_evidence(&workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "nextjs_route_evidence" => {
                if !has_nextjs_route_evidence(&workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "requested_content_evidence" => {
                if !workspace.readme && required_paths.iter().all(|path| !path.ends_with(".md")) {
                    missing_evidence.push(evidence.clone());
                }
            }
            unknown => missing_evidence.push(format!("unsupported_required_evidence:{unknown}")),
        }
    }

    collect_weak_verify_evidence(verify_commands, &workspace, &mut weak_evidence);
    collect_weak_obligation_evidence(&artifact_obligations, &required, &mut weak_evidence);
    let missing_obligations =
        missing_required_obligations(required_obligations, &artifact_obligations, &workspace);
    let capability_evidence_bindings = capability_evidence_bindings(
        required_capabilities,
        &artifact_obligations,
        &missing_evidence,
    );
    let obligation_repair_targets = obligation_repair_targets(
        required_paths,
        &artifact_obligations,
        &workspace,
        &missing_obligations,
    );
    let inconclusive = !inconclusive_reasons.is_empty();
    let weak_evidence_blocks_completion = !weak_evidence.is_empty()
        && source_first_completion_authority_required(
            required_capabilities,
            required_evidence,
            required_obligations,
        );
    let passed = missing_capabilities.is_empty()
        && missing_evidence.is_empty()
        && missing_obligations.is_empty()
        && !inconclusive
        && !weak_evidence_blocks_completion;
    let primary_reason = if let Some(reason) = missing_capabilities.first() {
        format!("missing_required_capabilities:{reason}")
    } else if let Some(reason) = missing_evidence.first() {
        format!("missing_required_evidence:{reason}")
    } else if let Some(reason) = missing_obligations.first() {
        format!("missing_required_obligations:{reason}")
    } else if let Some(reason) = inconclusive_reasons.first() {
        format!("inconclusive_acceptance:{reason}")
    } else if let Some(reason) = weak_evidence.first() {
        format!("weak_verification_evidence:{reason}")
    } else {
        "pass".to_string()
    };
    let browser_interaction = browser_interaction.unwrap_or_default();

    RuntimeAcceptanceReport {
        passed,
        inconclusive,
        missing_capabilities,
        missing_evidence,
        missing_obligations,
        weak_evidence,
        inconclusive_reasons,
        artifact_obligations,
        capability_evidence_bindings,
        obligation_repair_targets,
        browser_readiness_status: browser_interaction.browser_readiness_status,
        browser_readiness_evidence_path: browser_interaction.browser_readiness_evidence_path,
        interaction_evidence_status: browser_interaction.interaction_evidence_status,
        interaction_evidence_path: browser_interaction.interaction_evidence_path,
        primary_reason,
    }
}

fn source_first_completion_authority_required(
    required_capabilities: &[String],
    required_evidence: &[String],
    required_obligations: &[String],
) -> bool {
    !required_capabilities.is_empty()
        || !required_evidence.is_empty()
        || !normalize_obligation_roles(required_obligations).is_empty()
}

pub fn artifact_obligation_evidence(
    root: &Path,
    required_paths: &[String],
    required_capabilities: &[String],
) -> Vec<ArtifactObligationEvidence> {
    let mut out = Vec::new();
    for path in required_paths {
        let full = root.join(path);
        if !full.is_file() {
            continue;
        }
        let content = std::fs::read_to_string(&full).unwrap_or_default();
        let file = SourceFile {
            rel: path.clone(),
            content,
        };
        let role = artifact_role_for_file(&file);
        let evidence = evidence_kinds_for_file(&file)
            .into_iter()
            .map(|kind| kind.as_str().to_string())
            .collect::<BTreeSet<_>>();
        let required_capabilities_supported = required_capabilities
            .iter()
            .filter(|capability| {
                evidence_kinds_for_capability(capability)
                    .into_iter()
                    .any(|kind| evidence.contains(kind.as_str()))
            })
            .cloned()
            .collect::<Vec<_>>();
        out.push(ArtifactObligationEvidence {
            path: path.clone(),
            role: role.as_str().to_string(),
            evidence: evidence.into_iter().collect(),
            satisfies_implementation: role.satisfies_implementation(),
            required_capabilities_supported,
        });
    }
    out
}

fn capability_evidence_bindings(
    required_capabilities: &[String],
    artifact_obligations: &[ArtifactObligationEvidence],
    missing_evidence: &[String],
) -> Vec<CapabilityEvidenceBinding> {
    let missing_set = missing_evidence.iter().cloned().collect::<BTreeSet<_>>();
    let mut bindings = Vec::new();
    for capability in required_capabilities {
        let capability = capability.trim();
        if capability.is_empty() {
            continue;
        }
        let required_evidence = evidence_kinds_for_capability(capability)
            .into_iter()
            .map(|kind| kind.as_str().to_string())
            .collect::<Vec<_>>();
        if required_evidence.is_empty() {
            continue;
        }
        let required_set = required_evidence.iter().cloned().collect::<BTreeSet<_>>();
        let satisfied_evidence = required_evidence
            .iter()
            .filter(|evidence| !missing_set.contains(*evidence))
            .cloned()
            .collect::<Vec<_>>();
        let capability_missing_evidence = required_evidence
            .iter()
            .filter(|evidence| missing_set.contains(*evidence))
            .cloned()
            .collect::<Vec<_>>();
        let artifact_paths = artifact_obligations
            .iter()
            .filter(|artifact| {
                artifact
                    .required_capabilities_supported
                    .iter()
                    .any(|supported| supported == capability)
                    || artifact
                        .evidence
                        .iter()
                        .any(|evidence| required_set.contains(evidence))
            })
            .map(|artifact| artifact.path.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        bindings.push(CapabilityEvidenceBinding {
            capability: capability.to_string(),
            required_evidence,
            satisfied_evidence,
            missing_evidence: capability_missing_evidence,
            artifact_paths,
        });
    }
    bindings
}

fn obligation_repair_targets(
    required_paths: &[String],
    artifact_obligations: &[ArtifactObligationEvidence],
    workspace: &WorkspaceEvidence,
    missing_obligations: &[String],
) -> Vec<ObligationRepairTarget> {
    missing_obligations
        .iter()
        .filter_map(|obligation| {
            let obligation = obligation.trim();
            if obligation.is_empty() {
                return None;
            }
            let (target_role, target_path, reason) = match obligation {
                "setup" => (
                    "setup",
                    setup_repair_path(required_paths, workspace),
                    "missing setup obligation; create or repair the project manifest",
                ),
                "scaffold" => (
                    "scaffold",
                    implementation_repair_path(required_paths, artifact_obligations, workspace),
                    "missing scaffold obligation; create the framework entrypoint shell",
                ),
                "implementation" => (
                    "implementation",
                    implementation_repair_path(required_paths, artifact_obligations, workspace),
                    "missing implementation obligation; replace setup/scaffold/style/docs-only output with executable source",
                ),
                "verification" => (
                    "verification",
                    verification_repair_path(required_paths, artifact_obligations, workspace),
                    "missing verification obligation; add a deterministic test or smoke artifact",
                ),
                "acceptance_evidence" => (
                    "acceptance_evidence",
                    "README.md".to_string(),
                    "missing acceptance evidence obligation; add task-specific acceptance notes",
                ),
                _ => return None,
            };
            Some(ObligationRepairTarget {
                obligation: obligation.to_string(),
                target_role: target_role.to_string(),
                target_path,
                reason: reason.to_string(),
            })
        })
        .collect()
}

fn setup_repair_path(required_paths: &[String], workspace: &WorkspaceEvidence) -> String {
    required_paths
        .iter()
        .find(|path| looks_like_setup_path(&path.to_ascii_lowercase()))
        .cloned()
        .unwrap_or_else(|| {
            if workspace.cargo_toml {
                "Cargo.toml".to_string()
            } else {
                "package.json".to_string()
            }
        })
}

fn implementation_repair_path(
    required_paths: &[String],
    artifact_obligations: &[ArtifactObligationEvidence],
    workspace: &WorkspaceEvidence,
) -> String {
    artifact_obligations
        .iter()
        .find(|artifact| {
            matches!(
                artifact.role.as_str(),
                "scaffold" | "style" | "acceptance_evidence"
            ) && looks_like_implementation_path(&artifact.path)
        })
        .map(|artifact| artifact.path.clone())
        .or_else(|| {
            required_paths
                .iter()
                .find(|path| looks_like_implementation_path(path))
                .cloned()
        })
        .or_else(|| {
            workspace
                .source_files
                .iter()
                .find(|file| looks_like_implementation_path(&file.rel))
                .map(|file| file.rel.clone())
        })
        .unwrap_or_else(|| {
            if workspace.package_json.is_some() {
                "src/app/page.tsx".to_string()
            } else if workspace.cargo_toml {
                "src/main.rs".to_string()
            } else {
                "src/main.rs".to_string()
            }
        })
}

fn verification_repair_path(
    required_paths: &[String],
    artifact_obligations: &[ArtifactObligationEvidence],
    workspace: &WorkspaceEvidence,
) -> String {
    required_paths
        .iter()
        .find(|path| looks_like_test_file(path))
        .cloned()
        .unwrap_or_else(|| {
            let implementation_path =
                implementation_repair_path(required_paths, artifact_obligations, workspace);
            let stem = implementation_path
                .rsplit('/')
                .next()
                .unwrap_or("main")
                .rsplit_once('.')
                .map_or(
                    "main",
                    |(stem, _)| if stem.is_empty() { "main" } else { stem },
                );
            if implementation_path.ends_with(".tsx") || implementation_path.ends_with(".ts") {
                format!("tests/{stem}.test.ts")
            } else if implementation_path.ends_with(".jsx") || implementation_path.ends_with(".js")
            {
                format!("tests/{stem}.test.js")
            } else if implementation_path.ends_with(".rs") {
                format!("tests/{stem}.rs")
            } else {
                format!("tests/test_{stem}.py")
            }
        })
}

fn looks_like_implementation_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if looks_like_setup_path(&lower)
        || looks_like_style_path(&lower)
        || looks_like_test_file(&lower)
        || lower.ends_with(".md")
        || lower.ends_with(".d.ts")
        || lower.ends_with("layout.tsx")
        || lower.ends_with("layout.jsx")
    {
        return false;
    }
    looks_like_source_or_test(&lower)
}

fn evidence_kinds_for_capability(capability: &str) -> Vec<EvidenceKind> {
    match capability.trim() {
        "implementation" | "entrypoint" | "input_output_contract" => {
            vec![EvidenceKind::ImplementationArtifact]
        }
        "requested_content" => vec![EvidenceKind::RequestedContent],
        "deterministic_test" => vec![EvidenceKind::TestArtifact, EvidenceKind::BoundVerifyCommand],
        "deterministic_check" => vec![
            EvidenceKind::BoundVerifyCommand,
            EvidenceKind::NonZeroTestOrAssertionEvidence,
        ],
        "buildable" => vec![EvidenceKind::BuildCommandOrDependencyBoundary],
        "browser_interaction" | "playable_ui" => vec![
            EvidenceKind::ImplementationArtifact,
            EvidenceKind::VisibleInteractiveSurfaceEvidence,
            EvidenceKind::UserInputHandlerEvidence,
            EvidenceKind::StatefulUpdateEvidence,
            EvidenceKind::InteractiveUiSourceEvidence,
            EvidenceKind::NonStaticScreenEvidence,
        ],
        "stateful_interaction" => vec![
            EvidenceKind::ImplementationArtifact,
            EvidenceKind::VisibleInteractiveSurfaceEvidence,
            EvidenceKind::UserInputHandlerEvidence,
            EvidenceKind::StatefulUpdateEvidence,
            EvidenceKind::InteractiveUiSourceEvidence,
            EvidenceKind::NonStaticScreenEvidence,
        ],
        "start_or_restart_flow" => vec![
            EvidenceKind::ImplementationArtifact,
            EvidenceKind::VisibleInteractiveSurfaceEvidence,
            EvidenceKind::UserInputHandlerEvidence,
            EvidenceKind::RestartOrRecoverableStateEvidence,
            EvidenceKind::InteractiveUiSourceEvidence,
        ],
        "player_control" | "user_input_or_action" => vec![
            EvidenceKind::ImplementationArtifact,
            EvidenceKind::VisibleInteractiveSurfaceEvidence,
            EvidenceKind::UserInputHandlerEvidence,
            EvidenceKind::InteractiveUiSourceEvidence,
        ],
        "adversary_or_challenge" => vec![
            EvidenceKind::ImplementationArtifact,
            EvidenceKind::ChallengeOrAdversaryEvidence,
            EvidenceKind::StatefulUpdateEvidence,
            EvidenceKind::NonStaticScreenEvidence,
        ],
        "progression_or_score" => vec![
            EvidenceKind::ImplementationArtifact,
            EvidenceKind::ScoreOrProgressionEvidence,
            EvidenceKind::StatefulUpdateEvidence,
            EvidenceKind::NonStaticScreenEvidence,
        ],
        "failure_or_collision_rule" => vec![
            EvidenceKind::ImplementationArtifact,
            EvidenceKind::FailureOrCollisionEvidence,
            EvidenceKind::StatefulUpdateEvidence,
            EvidenceKind::NonStaticScreenEvidence,
        ],
        "visible_state_change" => vec![
            EvidenceKind::ImplementationArtifact,
            EvidenceKind::VisibleInteractiveSurfaceEvidence,
            EvidenceKind::StatefulUpdateEvidence,
            EvidenceKind::InteractiveUiSourceEvidence,
        ],
        "nextjs_route" | "route" => vec![EvidenceKind::NextJsRouteEvidence],
        _ => Vec::new(),
    }
}

fn collect_workspace_evidence(root: &Path) -> WorkspaceEvidence {
    let mut evidence = WorkspaceEvidence::default();
    for path in collect_candidate_files(root) {
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel == "package.json" {
            evidence.package_json = std::fs::read_to_string(&path).ok();
        }
        if rel == "Cargo.toml" {
            evidence.cargo_toml = true;
        }
        if rel.eq_ignore_ascii_case("README.md") {
            evidence.readme = true;
        }
        if !looks_like_source_or_test(&rel) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let file = SourceFile {
            rel: rel.clone(),
            content,
        };
        if looks_like_test_file(&rel) {
            evidence.test_files.push(file.clone());
        }
        evidence.source_files.push(file);
    }
    evidence
}

fn collect_candidate_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if should_skip_entry(&name) {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                out.push(path);
            }
        }
    }
    out
}

fn should_skip_entry(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".anvil" | "target" | "node_modules" | ".next" | "dist" | "build"
    )
}

fn looks_like_source_or_test(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower.ends_with(".js")
        || lower.ends_with(".jsx")
        || lower.ends_with(".mjs")
        || lower.ends_with(".cjs")
        || lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".py")
        || lower.ends_with(".rs")
        || lower.ends_with(".md")
}

fn looks_like_test_file(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.contains("/test/")
        || lower.starts_with("test/")
        || lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.starts_with("test_")
        || lower.ends_with("_test.py")
        || lower.ends_with("_test.rs")
}

fn has_implementation_artifact(
    root: &Path,
    required_paths: &[String],
    workspace: &WorkspaceEvidence,
) -> bool {
    required_paths.iter().any(|path| {
        let full = root.join(path);
        if !full.is_file() {
            return false;
        }
        let content = std::fs::read_to_string(full).unwrap_or_default();
        artifact_role_for_file(&SourceFile {
            rel: path.clone(),
            content,
        })
        .satisfies_implementation()
    }) || workspace
        .source_files
        .iter()
        .any(|file| artifact_role_for_file(file).satisfies_implementation())
}

fn has_test_artifact(workspace: &WorkspaceEvidence) -> bool {
    !workspace.test_files.is_empty()
        || workspace
            .source_files
            .iter()
            .any(|file| has_inline_test_or_self_test(file))
}

fn has_assertion_or_test_evidence(workspace: &WorkspaceEvidence) -> bool {
    workspace.source_files.iter().any(|file| {
        has_inline_test_or_self_test(file)
            || contains_assertion(&file.content)
            || file.content.contains("#[test]")
    })
}

fn has_inline_test_or_self_test(file: &SourceFile) -> bool {
    let content = file.content.as_str();
    if file.rel.ends_with(".rs") {
        return content.contains("#[test]") || content.contains("#[cfg(test)]");
    }
    if file.rel.ends_with(".py") {
        return (content.contains("def test_") || content.contains("unittest.TestCase"))
            && contains_assertion(content);
    }
    if file.rel.ends_with(".js")
        || file.rel.ends_with(".mjs")
        || file.rel.ends_with(".cjs")
        || file.rel.ends_with(".ts")
        || file.rel.ends_with(".tsx")
        || file.rel.ends_with(".jsx")
    {
        return (content.contains("node:assert")
            || content.contains("require(\"assert\")")
            || content.contains("require('assert')")
            || content.contains("assert."))
            && contains_assertion(content);
    }
    false
}

fn contains_assertion(content: &str) -> bool {
    content.contains("assert")
        || content.contains("expect(")
        || content.contains("should")
        || content.contains("assert_eq!")
        || content.contains("assert_ne!")
}

fn has_bound_verify_command(verify_commands: &[String], workspace: &WorkspaceEvidence) -> bool {
    verify_commands
        .iter()
        .any(|command| verify_command_kind(command, workspace).is_strong_for_capability())
}

fn has_build_command_or_dependency_boundary(
    verify_commands: &[String],
    deferred_verify_requirements: &[String],
    workspace: &WorkspaceEvidence,
) -> bool {
    verify_commands
        .iter()
        .any(|command| verify_command_kind(command, workspace).is_build())
        || deferred_verify_requirements
            .iter()
            .any(|command| verify_command_kind(command, workspace).is_build())
        || package_json_has_next_build_script(workspace)
}

fn collect_weak_verify_evidence(
    verify_commands: &[String],
    workspace: &WorkspaceEvidence,
    weak: &mut Vec<String>,
) {
    for command in verify_commands {
        match verify_command_kind(command, workspace) {
            VerifyCommandKind::Weak(reason) => weak.push(reason),
            VerifyCommandKind::ArtifactOnly => weak.push(format!("artifact_only_verify:{command}")),
            _ => {}
        }
    }
    weak.sort();
    weak.dedup();
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum VerifyCommandKind {
    Test,
    Build,
    StaticSyntax,
    ArtifactOnly,
    Weak(String),
    Other,
}

impl VerifyCommandKind {
    fn is_strong_for_capability(&self) -> bool {
        matches!(self, Self::Test | Self::Build | Self::StaticSyntax)
    }

    fn is_build(&self) -> bool {
        matches!(self, Self::Build)
    }
}

fn verify_command_kind(command: &str, workspace: &WorkspaceEvidence) -> VerifyCommandKind {
    let lower = command.trim().to_ascii_lowercase();
    if lower.starts_with("test -f ") || lower.starts_with("cat ") {
        return VerifyCommandKind::ArtifactOnly;
    }
    if lower == "npm run build"
        || lower == "pnpm build"
        || lower == "yarn build"
        || lower == "cargo build"
        || lower.starts_with("cargo build ")
    {
        return VerifyCommandKind::Build;
    }
    if lower.starts_with("python3 -m py_compile ") || lower.starts_with("python -m py_compile ") {
        return VerifyCommandKind::StaticSyntax;
    }
    if lower == "cargo test" || lower.starts_with("cargo test ") {
        if has_assertion_or_test_evidence(workspace) {
            return VerifyCommandKind::Test;
        }
        return VerifyCommandKind::Weak("cargo_test_without_test_evidence".to_string());
    }
    if lower == "npm test"
        || lower == "npm run test"
        || lower == "pnpm test"
        || lower == "yarn test"
    {
        if has_test_artifact(workspace) {
            return VerifyCommandKind::Test;
        }
        return VerifyCommandKind::Weak("node_test_without_test_artifact".to_string());
    }
    if lower.starts_with("python3 -m unittest") || lower.starts_with("python -m unittest") {
        if has_test_artifact(workspace) {
            return VerifyCommandKind::Test;
        }
        return VerifyCommandKind::Weak("unittest_without_test_artifact".to_string());
    }
    if lower.starts_with("node ") {
        if has_assertion_or_test_evidence(workspace) {
            return VerifyCommandKind::Test;
        }
        return VerifyCommandKind::Weak("node_smoke_without_assertion".to_string());
    }
    VerifyCommandKind::Other
}

fn has_interactive_ui_source(workspace: &WorkspaceEvidence) -> bool {
    workspace
        .source_files
        .iter()
        .any(source_file_has_interactive_ui)
}

fn has_non_static_screen_evidence(workspace: &WorkspaceEvidence) -> bool {
    workspace
        .source_files
        .iter()
        .any(source_file_has_non_static_screen)
}

fn has_visible_interactive_surface_evidence(workspace: &WorkspaceEvidence) -> bool {
    workspace
        .source_files
        .iter()
        .any(source_file_has_visible_interactive_surface)
}

fn has_user_input_handler_evidence(workspace: &WorkspaceEvidence) -> bool {
    workspace
        .source_files
        .iter()
        .any(source_file_has_user_input_handler)
}

fn has_stateful_update_evidence(workspace: &WorkspaceEvidence) -> bool {
    workspace
        .source_files
        .iter()
        .any(source_file_has_stateful_update)
}

fn has_challenge_or_adversary_evidence(workspace: &WorkspaceEvidence) -> bool {
    workspace
        .source_files
        .iter()
        .any(source_file_has_challenge_or_adversary)
}

fn has_score_or_progression_evidence(workspace: &WorkspaceEvidence) -> bool {
    workspace
        .source_files
        .iter()
        .any(source_file_has_score_or_progression)
}

fn has_failure_or_collision_evidence(workspace: &WorkspaceEvidence) -> bool {
    workspace
        .source_files
        .iter()
        .any(source_file_has_failure_or_collision)
}

fn has_restart_or_recoverable_state_evidence(workspace: &WorkspaceEvidence) -> bool {
    workspace
        .source_files
        .iter()
        .any(source_file_has_restart_or_recoverable_state)
}

fn has_nextjs_route_evidence(workspace: &WorkspaceEvidence) -> bool {
    workspace.source_files.iter().any(|file| {
        let path = file.rel.to_ascii_lowercase();
        matches!(
            path.as_str(),
            "src/app/page.tsx"
                | "src/app/page.jsx"
                | "app/page.tsx"
                | "app/page.jsx"
                | "pages/index.tsx"
                | "pages/index.jsx"
                | "pages/index.js"
        ) && !file.content.trim().is_empty()
    })
}

fn package_json_has_next_build_script(workspace: &WorkspaceEvidence) -> bool {
    let Some(package_json) = workspace.package_json.as_ref() else {
        return false;
    };
    let Ok(package): Result<serde_json::Value, _> = serde_json::from_str(package_json) else {
        return false;
    };
    package
        .get("scripts")
        .and_then(serde_json::Value::as_object)
        .and_then(|scripts| scripts.get("build"))
        .and_then(serde_json::Value::as_str)
        == Some("next build")
}

fn artifact_role_for_file(file: &SourceFile) -> ArtifactRoleLite {
    let lower_path = file.rel.to_ascii_lowercase();
    if looks_like_setup_path(&lower_path) {
        return ArtifactRoleLite::Setup;
    }
    if looks_like_style_path(&lower_path) {
        return ArtifactRoleLite::Style;
    }
    if looks_like_test_file(&file.rel) {
        return ArtifactRoleLite::Verification;
    }
    if lower_path.ends_with(".md") {
        return ArtifactRoleLite::AcceptanceEvidence;
    }
    if looks_like_scaffold_file(file) {
        return ArtifactRoleLite::Scaffold;
    }
    if looks_like_source_or_test(&file.rel) && !file.content.trim().is_empty() {
        return ArtifactRoleLite::Implementation;
    }
    ArtifactRoleLite::Scaffold
}

fn looks_like_setup_path(lower_path: &str) -> bool {
    matches!(
        lower_path,
        "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lockb"
            | "cargo.toml"
            | "cargo.lock"
            | "pyproject.toml"
            | "requirements.txt"
            | "tsconfig.json"
            | "next.config.js"
            | "next.config.mjs"
            | "next.config.ts"
            | "postcss.config.js"
            | "tailwind.config.js"
            | "tailwind.config.ts"
            | "vite.config.js"
            | "vite.config.ts"
    ) || lower_path.ends_with(".d.ts")
}

fn looks_like_style_path(lower_path: &str) -> bool {
    lower_path.ends_with(".css")
        || lower_path.ends_with(".scss")
        || lower_path.ends_with(".sass")
        || lower_path.ends_with(".less")
}

fn looks_like_scaffold_file(file: &SourceFile) -> bool {
    let lower_path = file.rel.to_ascii_lowercase();
    let lower = file.content.to_ascii_lowercase();
    if file.content.trim().is_empty() {
        return true;
    }
    if lower_path.ends_with("layout.tsx") || lower_path.ends_with("layout.jsx") {
        return true;
    }
    let placeholder = [
        "todo",
        "placeholder",
        "coming soon",
        "hello world",
        "press any key",
        "start screen",
        "title screen",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    placeholder && !source_file_has_interactive_ui(file) && !source_file_has_non_static_screen(file)
}

fn evidence_kinds_for_file(file: &SourceFile) -> Vec<EvidenceKind> {
    let mut kinds = Vec::new();
    match artifact_role_for_file(file) {
        ArtifactRoleLite::Implementation => kinds.push(EvidenceKind::ImplementationArtifact),
        ArtifactRoleLite::Verification => {
            kinds.push(EvidenceKind::TestArtifact);
            if has_inline_test_or_self_test(file) || contains_assertion(&file.content) {
                kinds.push(EvidenceKind::NonZeroTestOrAssertionEvidence);
            }
        }
        ArtifactRoleLite::AcceptanceEvidence => kinds.push(EvidenceKind::RequestedContent),
        ArtifactRoleLite::Setup | ArtifactRoleLite::Scaffold | ArtifactRoleLite::Style => {}
    }
    if has_inline_test_or_self_test(file) || contains_assertion(&file.content) {
        kinds.push(EvidenceKind::TestArtifact);
        kinds.push(EvidenceKind::NonZeroTestOrAssertionEvidence);
    }
    if source_file_has_interactive_ui(file) {
        kinds.push(EvidenceKind::InteractiveUiSourceEvidence);
    }
    if source_file_has_non_static_screen(file) {
        kinds.push(EvidenceKind::NonStaticScreenEvidence);
    }
    if source_file_has_visible_interactive_surface(file) {
        kinds.push(EvidenceKind::VisibleInteractiveSurfaceEvidence);
    }
    if source_file_has_user_input_handler(file) {
        kinds.push(EvidenceKind::UserInputHandlerEvidence);
    }
    if source_file_has_stateful_update(file) {
        kinds.push(EvidenceKind::StatefulUpdateEvidence);
    }
    if source_file_has_challenge_or_adversary(file) {
        kinds.push(EvidenceKind::ChallengeOrAdversaryEvidence);
    }
    if source_file_has_score_or_progression(file) {
        kinds.push(EvidenceKind::ScoreOrProgressionEvidence);
    }
    if source_file_has_failure_or_collision(file) {
        kinds.push(EvidenceKind::FailureOrCollisionEvidence);
    }
    if source_file_has_restart_or_recoverable_state(file) {
        kinds.push(EvidenceKind::RestartOrRecoverableStateEvidence);
    }
    kinds.sort_by_key(|kind| kind.as_str());
    kinds.dedup();
    kinds
}

fn collect_weak_obligation_evidence(
    obligations: &[ArtifactObligationEvidence],
    required_evidence: &BTreeSet<String>,
    weak: &mut Vec<String>,
) {
    if !required_evidence.contains("implementation_artifact") {
        return;
    }
    if obligations
        .iter()
        .any(|obligation| obligation.satisfies_implementation)
    {
        return;
    }
    let roles = obligations
        .iter()
        .map(|obligation| obligation.role.as_str())
        .collect::<BTreeSet<_>>();
    for role in roles {
        weak.push(format!("non_implementation_obligation_only:{role}"));
    }
}

fn missing_required_obligations(
    required_obligations: &[String],
    artifact_obligations: &[ArtifactObligationEvidence],
    workspace: &WorkspaceEvidence,
) -> Vec<String> {
    let mut missing = Vec::new();
    for obligation in normalize_obligation_roles(required_obligations) {
        if !obligation_role_satisfied(&obligation, artifact_obligations, workspace) {
            missing.push(obligation);
        }
    }
    missing
}

fn normalize_obligation_roles(required_obligations: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for obligation in required_obligations {
        let normalized = obligation.trim().to_ascii_lowercase().replace('-', "_");
        if matches!(
            normalized.as_str(),
            "setup" | "scaffold" | "implementation" | "verification" | "acceptance_evidence"
        ) && seen.insert(normalized.clone())
        {
            out.push(normalized);
        }
    }
    out
}

fn obligation_role_satisfied(
    role: &str,
    artifact_obligations: &[ArtifactObligationEvidence],
    workspace: &WorkspaceEvidence,
) -> bool {
    match role {
        "setup" => workspace.package_json.is_some() || workspace.cargo_toml,
        "scaffold" => {
            artifact_obligations
                .iter()
                .any(|obligation| obligation.role == "scaffold")
                || workspace
                    .source_files
                    .iter()
                    .any(|file| artifact_role_for_file(file) == ArtifactRoleLite::Scaffold)
        }
        "implementation" => {
            artifact_obligations
                .iter()
                .any(|obligation| obligation.satisfies_implementation)
                || workspace
                    .source_files
                    .iter()
                    .any(|file| artifact_role_for_file(file).satisfies_implementation())
        }
        "verification" => has_test_artifact(workspace) || has_assertion_or_test_evidence(workspace),
        "acceptance_evidence" => {
            workspace.readme
                || artifact_obligations
                    .iter()
                    .any(|obligation| obligation.role == "acceptance_evidence")
        }
        _ => false,
    }
}

fn source_file_has_interactive_ui(file: &SourceFile) -> bool {
    let content = file.content.as_str();
    let lower = content.to_ascii_lowercase();
    (content.contains("useState")
        || content.contains("useReducer")
        || content.contains("addEventListener")
        || content.contains("onKeyDown")
        || content.contains("onClick")
        || content.contains("requestAnimationFrame")
        || lower.contains("<canvas"))
        && (lower.contains("keydown")
            || lower.contains("arrow")
            || lower.contains("click")
            || lower.contains("pointer")
            || lower.contains("touch")
            || lower.contains("canvas"))
}

fn source_file_has_non_static_screen(file: &SourceFile) -> bool {
    let lower = file.content.to_ascii_lowercase();
    (lower.contains("score")
        || lower.contains("level")
        || lower.contains("life")
        || lower.contains("lives")
        || lower.contains("enemy")
        || lower.contains("invader")
        || lower.contains("collision")
        || lower.contains("bullet")
        || lower.contains("shot")
        || lower.contains("state"))
        && (lower.contains("setinterval")
            || lower.contains("requestanimationframe")
            || lower.contains("usestate")
            || lower.contains("usereducer")
            || lower.contains("canvas"))
}

fn source_file_has_visible_interactive_surface(file: &SourceFile) -> bool {
    let lower = file.content.to_ascii_lowercase();
    lower.contains("<canvas")
        || lower.contains("<button")
        || lower.contains("<input")
        || lower.contains("<select")
        || lower.contains("<textarea")
        || lower.contains("onclick")
        || lower.contains("onkeydown")
        || lower.contains("onpointer")
        || lower.contains("role=\"button\"")
        || lower.contains("role='button'")
        || lower.contains("tabindex")
}

fn source_file_has_user_input_handler(file: &SourceFile) -> bool {
    let content = file.content.as_str();
    let lower = content.to_ascii_lowercase();
    content.contains("addEventListener")
        || lower.contains("onkeydown")
        || lower.contains("onkeyup")
        || lower.contains("onclick")
        || lower.contains("onpointer")
        || lower.contains("onmousedown")
        || lower.contains("onmouseup")
        || lower.contains("ontouch")
        || lower.contains("onsubmit")
        || lower.contains("onchange")
        || lower.contains("keydown")
        || lower.contains("keyup")
        || lower.contains("pointerdown")
        || lower.contains("touchstart")
}

fn source_file_has_stateful_update(file: &SourceFile) -> bool {
    let content = file.content.as_str();
    let lower = content.to_ascii_lowercase();
    content.contains("useState")
        || content.contains("useReducer")
        || content.contains("setState")
        || lower.contains("setinterval")
        || lower.contains("settimeout")
        || lower.contains("requestanimationframe")
        || lower.contains("dispatch(")
        || lower.contains("=> set")
}

fn source_file_has_challenge_or_adversary(file: &SourceFile) -> bool {
    let lower = file.content.to_ascii_lowercase();
    [
        "enemy",
        "enemies",
        "adversary",
        "opponent",
        "obstacle",
        "hazard",
        "wave",
        "spawn",
        "target",
        "challenge",
        "boss",
        "敵",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn source_file_has_score_or_progression(file: &SourceFile) -> bool {
    let lower = file.content.to_ascii_lowercase();
    [
        "score",
        "points",
        "level",
        "stage",
        "wave",
        "combo",
        "lives",
        "life",
        "health",
        "progress",
        "スコア",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn source_file_has_failure_or_collision(file: &SourceFile) -> bool {
    let lower = file.content.to_ascii_lowercase();
    let has_failure_token = [
        "collision",
        "collide",
        "hit",
        "damage",
        "gameover",
        "game over",
        "lives",
        "life",
        "health",
        "intersect",
        "overlap",
        "bounds",
        "lose",
        "fail",
        "衝突",
        "当たり",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    if !has_failure_token {
        return false;
    }
    let failure_state_transition = [
        "setgamestate(\"gameover\"",
        "setgamestate('gameover'",
        "setgamestate(`gameover`",
        "setgamestate(\"game over\"",
        "setgamestate('game over'",
        "setgamestate(\"lost\"",
        "setgamestate('lost'",
        "setstatus(\"gameover\"",
        "setstatus('gameover'",
        "setstatus(\"lost\"",
        "setstatus('lost'",
        "setscreen(\"gameover\"",
        "setscreen('gameover'",
        "setmode(\"gameover\"",
        "setmode('gameover'",
        "dispatch({type:\"gameover\"",
        "dispatch({ type: \"gameover\"",
        "dispatch({type:'gameover'",
        "dispatch({ type: 'gameover'",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let damage_or_life_mutation = [
        "setlives(",
        "setlife(",
        "sethealth(",
        "sethp(",
        "lives -",
        "life -",
        "health -",
        "hp -",
        "lives--",
        "health--",
        "damageplayer(",
        "takedamage(",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    failure_state_transition || damage_or_life_mutation
}

fn source_file_has_restart_or_recoverable_state(file: &SourceFile) -> bool {
    let lower = file.content.to_ascii_lowercase();
    let has_recoverable_state = [
        "start",
        "restart",
        "reset",
        "pause",
        "resume",
        "gameover",
        "game over",
        "play again",
        "try again",
        "スタート",
        "開始",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let has_recoverable_transition = [
        "setgamestate(",
        "setstatus(",
        "setscreen(",
        "setmode(",
        "dispatch(",
        "resetgame(",
        "restartgame(",
        "startgame(",
        "newgame(",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    has_recoverable_state && has_recoverable_transition && source_file_has_user_input_handler(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_exports_only_missing_deterministic_test() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("date-helper.js"),
            "exports.formatDate = (d) => String(d);\n",
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["date-helper.js".to_string()],
            &["node date-helper.js".to_string()],
            &[
                "implementation".to_string(),
                "deterministic_test".to_string(),
            ],
            &[],
            &[],
            &[],
        );
        assert!(!report.passed);
        assert!(
            report
                .missing_evidence
                .contains(&"test_artifact".to_string())
        );
        assert!(
            report
                .missing_evidence
                .contains(&"bound_verify_command".to_string())
        );
        assert!(
            report
                .weak_evidence
                .contains(&"node_smoke_without_assertion".to_string())
        );
    }

    #[test]
    fn js_self_test_satisfies_deterministic_test() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("date-helper.js"),
            "const assert = require('assert');\nexports.addDays = () => 1;\nassert.equal(exports.addDays(), 1);\n",
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["date-helper.js".to_string()],
            &["node date-helper.js".to_string()],
            &[
                "implementation".to_string(),
                "deterministic_test".to_string(),
            ],
            &[],
            &[],
            &[],
        );
        assert!(report.passed, "{report:?}");
    }

    #[test]
    fn rust_hello_world_missing_deterministic_check() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname=\"x\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/main.rs"),
            "fn main(){println!(\"Hello, world!\");}\n",
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["Cargo.toml".to_string(), "src/main.rs".to_string()],
            &["cargo test".to_string()],
            &["entrypoint".to_string(), "deterministic_check".to_string()],
            &[],
            &[],
            &[],
        );
        assert!(!report.passed);
        assert!(
            report
                .missing_evidence
                .contains(&"non_zero_test_or_assertion_evidence".to_string())
        );
    }

    #[test]
    fn rust_inline_test_satisfies_deterministic_check() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname=\"x\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/main.rs"),
            "fn main(){}\n#[cfg(test)] mod tests { #[test] fn it_works(){ assert_eq!(2, 2); } }\n",
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["Cargo.toml".to_string(), "src/main.rs".to_string()],
            &["cargo test".to_string()],
            &["entrypoint".to_string(), "deterministic_check".to_string()],
            &[],
            &[],
            &[],
        );
        assert!(report.passed, "{report:?}");
    }

    #[test]
    fn interactive_game_requires_dynamic_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){ return <main>Press any key to start</main>; }\n",
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &[
                "player_control".to_string(),
                "progression_or_score".to_string(),
            ],
            &[],
            &[],
            &["npm run build".to_string()],
        );
        assert!(!report.passed);
        assert!(
            report
                .missing_evidence
                .contains(&"interactive_ui_source_evidence".to_string())
        );
    }

    #[test]
    fn setup_only_does_not_satisfy_implementation_obligation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"next build"}}"#,
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["package.json".to_string()],
            &[],
            &["implementation".to_string()],
            &[],
            &[],
            &[],
        );
        assert!(!report.passed);
        assert!(
            report
                .missing_evidence
                .contains(&"implementation_artifact".to_string())
        );
        assert_eq!(report.artifact_obligations[0].role, "setup");
        assert!(
            report
                .weak_evidence
                .contains(&"non_implementation_obligation_only:setup".to_string())
        );
    }

    #[test]
    fn explicit_implementation_obligation_rejects_setup_only_output() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"scripts":{}}"#).unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["package.json".to_string()],
            &[],
            &[],
            &[],
            &["implementation".to_string()],
            &[],
        );
        assert!(!report.passed);
        assert!(
            report
                .missing_obligations
                .contains(&"implementation".to_string())
        );
        assert!(
            report
                .primary_reason
                .contains("missing_required_obligations")
        );
        assert_eq!(report.obligation_repair_targets.len(), 1);
        assert_eq!(
            report.obligation_repair_targets[0].obligation,
            "implementation"
        );
        assert_eq!(
            report.obligation_repair_targets[0].target_role,
            "implementation"
        );
        assert_eq!(
            report.obligation_repair_targets[0].target_path,
            "src/app/page.tsx"
        );
    }

    #[test]
    fn scaffold_only_does_not_satisfy_implementation_obligation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){ return <main>Press any key to start</main>; }\n",
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &["implementation".to_string()],
            &[],
            &[],
            &[],
        );
        assert!(!report.passed);
        assert_eq!(report.artifact_obligations[0].role, "scaffold");
    }

    #[test]
    fn style_only_does_not_satisfy_implementation_obligation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "body { color: white; }\n",
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/globals.css".to_string()],
            &[],
            &["implementation".to_string()],
            &[],
            &[],
            &[],
        );
        assert!(!report.passed);
        assert_eq!(report.artifact_obligations[0].role, "style");
    }

    #[test]
    fn docs_only_output_does_not_satisfy_game_acceptance() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "# Game\nUse arrow keys.\n").unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["README.md".to_string()],
            &[],
            &[
                "player_control".to_string(),
                "adversary_or_challenge".to_string(),
            ],
            &[],
            &[],
            &[],
        );
        assert!(!report.passed);
        assert_eq!(report.artifact_obligations[0].role, "acceptance_evidence");
        assert!(
            report
                .missing_evidence
                .contains(&"implementation_artifact".to_string())
        );
    }

    #[test]
    fn title_only_output_does_not_satisfy_interactive_game_evidence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){ return <main><h1>Game</h1><p>Press any key to start</p></main>; }\n",
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &[
                "stateful_interaction".to_string(),
                "start_or_restart_flow".to_string(),
                "player_control".to_string(),
                "adversary_or_challenge".to_string(),
                "progression_or_score".to_string(),
                "failure_or_collision_rule".to_string(),
            ],
            &[],
            &["implementation".to_string()],
            &[],
        );
        assert!(!report.passed);
        assert!(
            report
                .missing_evidence
                .contains(&"visible_interactive_surface_evidence".to_string())
        );
        assert!(
            report
                .missing_evidence
                .contains(&"user_input_handler_evidence".to_string())
        );
        assert!(
            report
                .missing_evidence
                .contains(&"restart_or_recoverable_state_evidence".to_string())
        );
    }

    #[test]
    fn unreachable_game_state_literals_do_not_satisfy_release_grade_game_evidence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useEffect, useState } from "react";
type GameState = "playing" | "gameover" | "win";
export default function Page(){
  const [score, setScore] = useState(0);
  const [gameState, setGameState] = useState<GameState>("playing");
  const enemies = [{ x: 10, y: 20 }];
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") setScore((value) => value + 1);
    };
    const frame = requestAnimationFrame(() => {
      const collision = enemies.some((enemy) => enemy.x > 0);
      if (collision) setScore((value) => value + 10);
    });
    window.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);
  return <main><canvas /><p>score {score}</p><p>{gameState}</p><button>Restart</button></main>;
}
"#,
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &[
                "stateful_interaction".to_string(),
                "start_or_restart_flow".to_string(),
                "player_control".to_string(),
                "adversary_or_challenge".to_string(),
                "progression_or_score".to_string(),
                "failure_or_collision_rule".to_string(),
            ],
            &[],
            &["implementation".to_string()],
            &[],
        );
        assert!(!report.passed);
        assert!(
            report
                .missing_evidence
                .contains(&"failure_or_collision_evidence".to_string())
        );
        assert!(
            report
                .missing_evidence
                .contains(&"restart_or_recoverable_state_evidence".to_string())
        );
    }

    #[test]
    fn verification_and_report_artifacts_do_not_satisfy_implementation_obligation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.test.tsx"),
            r#"import { expect, test } from "vitest";
test("documents intended gameplay", () => {
  const page = "canvas button score enemy gameover";
  expect(page).toContain("score");
});
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("README.md"),
            "Report: the game should have canvas controls, enemies, score, and restart.\n",
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.test.tsx".to_string(), "README.md".to_string()],
            &[],
            &["player_control".to_string()],
            &[],
            &["implementation".to_string()],
            &[],
        );
        assert!(!report.passed);
        assert!(
            report
                .missing_evidence
                .contains(&"implementation_artifact".to_string())
        );
        assert!(
            report
                .missing_obligations
                .contains(&"implementation".to_string())
        );
        assert!(
            report
                .artifact_obligations
                .iter()
                .all(|artifact| !artifact.satisfies_implementation)
        );
    }

    #[test]
    fn interactive_game_source_satisfies_generic_capability_evidence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useEffect, useState } from "react";
export default function Page(){
  const [score, setScore] = useState(0);
  const [gameState, setGameState] = useState("ready");
  const enemies = [{ x: 10, y: 20 }];
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") {
        setGameState("playing");
        setScore((value) => value + 1);
      }
    };
    const frame = requestAnimationFrame(() => {
      const collision = enemies.some((enemy) => enemy.x > 0);
      if (collision) setGameState("gameover");
    });
    window.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);
  return <main><button onClick={() => setGameState("playing")}>Start</button><button onClick={() => { setGameState("ready"); setScore(0); }}>Restart</button><canvas /><p>score {score} enemy collision {gameState}</p></main>;
}
"#,
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &[
                "stateful_interaction".to_string(),
                "start_or_restart_flow".to_string(),
                "player_control".to_string(),
                "adversary_or_challenge".to_string(),
                "progression_or_score".to_string(),
                "failure_or_collision_rule".to_string(),
            ],
            &[],
            &["implementation".to_string()],
            &[],
        );
        assert!(report.passed, "{report:?}");
        let evidence = &report.artifact_obligations[0].evidence;
        assert!(evidence.contains(&"visible_interactive_surface_evidence".to_string()));
        assert!(evidence.contains(&"user_input_handler_evidence".to_string()));
        assert!(evidence.contains(&"stateful_update_evidence".to_string()));
        assert!(evidence.contains(&"challenge_or_adversary_evidence".to_string()));
        assert!(evidence.contains(&"score_or_progression_evidence".to_string()));
        assert!(evidence.contains(&"failure_or_collision_evidence".to_string()));
        assert!(evidence.contains(&"restart_or_recoverable_state_evidence".to_string()));
    }

    #[test]
    fn artifact_only_verify_does_not_satisfy_source_first_implementation_contract() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useEffect, useState } from "react";
export default function Page(){
  const [score, setScore] = useState(0);
  const [gameState, setGameState] = useState("ready");
  const enemies = [{ x: 10, y: 20 }];
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") {
        setGameState("playing");
        setScore((value) => value + 1);
      }
    };
    const frame = requestAnimationFrame(() => {
      const collision = enemies.some((enemy) => enemy.x > 0);
      if (collision) setGameState("gameover");
    });
    window.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);
  return <main><button onClick={() => setGameState("playing")}>Start</button><canvas /><p>score {score} enemy collision {gameState}</p></main>;
}
"#,
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &["test -f src/app/page.tsx".to_string()],
            &[
                "stateful_interaction".to_string(),
                "player_control".to_string(),
                "progression_or_score".to_string(),
                "failure_or_collision_rule".to_string(),
            ],
            &[],
            &["implementation".to_string()],
            &[],
        );
        assert!(!report.passed);
        assert!(report.missing_capabilities.is_empty(), "{report:?}");
        assert!(report.missing_evidence.is_empty(), "{report:?}");
        assert!(report.missing_obligations.is_empty(), "{report:?}");
        assert!(
            report
                .weak_evidence
                .contains(&"artifact_only_verify:test -f src/app/page.tsx".to_string())
        );
        assert!(
            report.primary_reason.contains("weak_verification_evidence"),
            "{report:?}"
        );
    }

    #[test]
    fn required_capability_maps_to_expected_artifact_evidence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useEffect, useState } from "react";
export default function Page(){
  const [score, setScore] = useState(0);
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") setScore((value) => value + 1);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
  return <canvas data-score={score} />;
}
"#,
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &["player_control".to_string()],
            &[],
            &[],
            &[],
        );
        assert!(report.artifact_obligations[0].satisfies_implementation);
        assert!(
            report.artifact_obligations[0]
                .evidence
                .contains(&"interactive_ui_source_evidence".to_string())
        );
        assert!(
            report.artifact_obligations[0]
                .required_capabilities_supported
                .contains(&"player_control".to_string())
        );
        let binding = report
            .capability_evidence_bindings
            .iter()
            .find(|binding| binding.capability == "player_control")
            .expect("player_control binding");
        assert!(
            binding
                .artifact_paths
                .contains(&"src/app/page.tsx".to_string())
        );
        assert!(
            binding
                .required_evidence
                .contains(&"visible_interactive_surface_evidence".to_string())
        );
        assert!(binding.missing_evidence.is_empty());
    }

    #[test]
    fn missing_capability_binding_points_at_partial_artifact_evidence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useState } from "react";
export default function Page(){
  const [score, setScore] = useState(0);
  return <main><button onClick={() => setScore(score + 1)}>Add</button><p>score {score}</p></main>;
}
"#,
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &["adversary_or_challenge".to_string()],
            &[],
            &["implementation".to_string()],
            &[],
        );
        assert!(!report.passed);
        let binding = report
            .capability_evidence_bindings
            .iter()
            .find(|binding| binding.capability == "adversary_or_challenge")
            .expect("adversary binding");
        assert!(
            binding
                .artifact_paths
                .contains(&"src/app/page.tsx".to_string())
        );
        assert!(
            binding
                .missing_evidence
                .contains(&"challenge_or_adversary_evidence".to_string())
        );
        assert!(
            binding
                .satisfied_evidence
                .contains(&"implementation_artifact".to_string())
        );
    }

    #[test]
    fn browser_interaction_is_inconclusive_without_browser_oracle() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useState } from "react";
export default function Page(){
  const [score,setScore] = useState(0);
  return <main><button onClick={() => setScore(score + 1)}>Go</button><p>score {score}</p></main>;
}
"#,
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &["browser_interaction".to_string()],
            &[],
            &[],
            &[],
        );
        assert!(!report.passed);
        assert!(report.inconclusive);
        assert!(report.inconclusive_reasons.contains(
            &"browser_readiness_unavailable:browser_readiness_evidence_missing".to_string()
        ));
        assert!(
            report.inconclusive_reasons.contains(
                &"browser_interaction_unavailable:interaction_evidence_missing".to_string()
            )
        );
    }

    #[test]
    fn browser_interaction_requires_render_and_interaction_evidence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useState } from "react";
export default function Page(){
  const [score,setScore] = useState(0);
  return <main><button onClick={() => setScore(score + 1)}>Go</button><p>score {score}</p></main>;
}
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200,"route_rendered":true}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("interaction-evidence.json"),
            r#"{"ok":true,"interaction_performed":true,"input_event_observed":true,"state_changed":true}"#,
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &["browser_interaction".to_string()],
            &[],
            &[],
            &[],
        );
        assert!(report.passed, "{report:?}");
        assert_eq!(report.browser_readiness_status, "passed");
        assert_eq!(report.interaction_evidence_status, "passed");
    }

    #[test]
    fn browser_http_500_fails_runtime_acceptance() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
export default function Page(){ return <button onClick={() => alert("ok")}>Go</button>; }
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":false,"http_status":500,"failure_kind":"browser_http_500"}"#,
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &["browser_interaction".to_string()],
            &[],
            &[],
            &[],
        );
        assert!(!report.passed);
        assert!(
            report
                .missing_evidence
                .contains(&"browser_readiness_failed:http_500".to_string())
        );
        assert_eq!(report.browser_readiness_status, "failed:http_500");
    }

    #[test]
    fn canvas_unavailable_fails_runtime_acceptance() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useState } from "react";
export default function Page(){
  const [score,setScore] = useState(0);
  return <main><canvas /><button onClick={() => setScore(score + 1)}>Go</button><p>score {score}</p></main>;
}
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200,"route_rendered":true}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("interaction-evidence.json"),
            r#"{"ok":true,"interaction_performed":true,"canvas_found":false}"#,
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &["browser_interaction".to_string()],
            &[],
            &[],
            &[],
        );
        assert!(!report.passed);
        assert!(
            report
                .missing_evidence
                .contains(&"browser_interaction_failed:canvas_unavailable".to_string())
        );
        assert_eq!(
            report.interaction_evidence_status,
            "failed:canvas_unavailable"
        );
    }

    #[test]
    fn nextjs_interactive_app_requires_route_and_build_contract_evidence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useEffect, useState } from "react";
export default function Page(){
  const [score,setScore] = useState(0);
  useEffect(() => {
    const onKeyDown = () => setScore((value) => value + 1);
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
  return <main><canvas /><p>enemy bullet collision score {score}</p></main>;
}
"#,
        )
        .unwrap();
        let missing_build = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &["player_control".to_string()],
            &[
                "nextjs_route_evidence".to_string(),
                "build_command_or_dependency_missing_boundary".to_string(),
            ],
            &["implementation".to_string()],
            &[],
        );
        assert!(!missing_build.passed);
        assert!(
            missing_build
                .missing_evidence
                .contains(&"build_command_or_dependency_missing_boundary".to_string())
        );

        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"next build"}}"#,
        )
        .unwrap();
        let passed = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &["player_control".to_string()],
            &[
                "nextjs_route_evidence".to_string(),
                "build_command_or_dependency_missing_boundary".to_string(),
            ],
            &["implementation".to_string()],
            &[],
        );
        assert!(passed.passed, "{passed:?}");
    }
}
