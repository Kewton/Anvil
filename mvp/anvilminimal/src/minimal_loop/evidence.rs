use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::minimal_loop::import_scan::route_bound_closure;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeAcceptanceReport {
    pub passed: bool,
    pub inconclusive: bool,
    pub missing_capabilities: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub missing_obligations: Vec<String>,
    pub weak_evidence: Vec<String>,
    pub diagnostics: Vec<String>,
    pub unverified_evidence: Vec<String>,
    pub evidence_tiers: BTreeMap<String, String>,
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
    pub route_bound: bool,
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
    PersistenceEvidence,
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
            Self::PersistenceEvidence => "persistence_evidence",
            Self::NextJsRouteEvidence => "nextjs_route_evidence",
            Self::RequestedContent => "requested_content_evidence",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SatisfactionChannel {
    SourceScan,
    TestArtifact,
    RuntimeArtifact,
}

pub fn evidence_satisfaction_channel(key: &str) -> SatisfactionChannel {
    let key = key
        .trim()
        .strip_prefix("unsupported_required_evidence:")
        .unwrap_or_else(|| key.trim())
        .split_once(':')
        .map_or_else(|| key.trim(), |(head, _)| head.trim());
    match key {
        "implementation_artifact"
        | "interactive_ui_source_evidence"
        | "non_static_screen_evidence"
        | "visible_interactive_surface_evidence"
        | "user_input_handler_evidence"
        | "stateful_update_evidence"
        | "challenge_or_adversary_evidence"
        | "score_or_progression_evidence"
        | "failure_or_collision_evidence"
        | "restart_or_recoverable_state_evidence"
        | "persistence_evidence"
        | "nextjs_route_evidence" => SatisfactionChannel::SourceScan,
        "test_artifact"
        | "bound_verify_command"
        | "non_zero_test_or_assertion_evidence"
        | "requested_content_evidence" => SatisfactionChannel::TestArtifact,
        _ => SatisfactionChannel::RuntimeArtifact,
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
    comments_stripped_strings_preserved: String,
    scan_content: String,
    route_bound: bool,
}

impl SourceFile {
    fn new(rel: String, content: String) -> Self {
        Self::new_with_route_bound(rel, content, true)
    }

    fn new_with_route_bound(rel: String, content: String, route_bound: bool) -> Self {
        let (comments_stripped_strings_preserved, scan_content) = source_scan_texts(&rel, &content);
        Self {
            rel,
            content,
            comments_stripped_strings_preserved,
            scan_content,
            route_bound,
        }
    }

    fn scan_text(&self) -> &str {
        &self.scan_content
    }

    fn comments_stripped_strings_preserved_text(&self) -> &str {
        &self.comments_stripped_strings_preserved
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiteralScanMode {
    Keep,
    Strip,
}

fn source_scan_texts(rel: &str, content: &str) -> (String, String) {
    if !uses_c_family_lexical_comments(rel) {
        return (content.to_string(), content.to_string());
    }
    let comments_stripped_strings_preserved =
        strip_c_family_comments_and_literals(content, LiteralScanMode::Keep)
            .unwrap_or_else(|| content.to_string());
    let scan = strip_c_family_comments_and_literals(content, LiteralScanMode::Strip)
        .unwrap_or_else(|| content.to_string());
    (comments_stripped_strings_preserved, scan)
}

fn uses_c_family_lexical_comments(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    [
        ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".c", ".cc", ".cpp", ".h", ".hpp", ".java",
        ".cs", ".go",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
}

fn strip_c_family_comments_and_literals(content: &str, mode: LiteralScanMode) -> Option<String> {
    let mut out = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                out.push(' ');
                out.push(' ');
                for comment_ch in chars.by_ref() {
                    if comment_ch == '\n' {
                        out.push('\n');
                        break;
                    }
                    out.push(' ');
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                out.push(' ');
                out.push(' ');
                let mut closed = false;
                let mut previous = '\0';
                for comment_ch in chars.by_ref() {
                    if comment_ch == '\n' {
                        out.push('\n');
                    } else {
                        out.push(' ');
                    }
                    if previous == '*' && comment_ch == '/' {
                        closed = true;
                        break;
                    }
                    previous = comment_ch;
                }
                if !closed {
                    return None;
                }
            }
            '\'' | '"' | '`' => {
                if !strip_or_copy_string_literal(ch, &mut chars, &mut out, mode) {
                    return None;
                }
            }
            _ => out.push(ch),
        }
    }
    Some(out)
}

fn strip_or_copy_string_literal<I>(
    quote: char,
    chars: &mut std::iter::Peekable<I>,
    out: &mut String,
    mode: LiteralScanMode,
) -> bool
where
    I: Iterator<Item = char>,
{
    out.push(quote);
    let mut escaped = false;
    for ch in chars.by_ref() {
        match mode {
            LiteralScanMode::Keep => out.push(ch),
            LiteralScanMode::Strip if ch == quote && !escaped => out.push(ch),
            LiteralScanMode::Strip if ch == '\n' => out.push('\n'),
            LiteralScanMode::Strip => out.push(' '),
        }

        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            return true;
        }
        if ch == '\n' && quote != '`' {
            return false;
        }
    }
    false
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
            "browser-interaction.json",
            "interaction-evidence.json",
            "interaction.json",
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
    if kind == BrowserEvidenceKind::Interaction
        && let Some(reason) = interaction_probe_unavailable_reason(root)
    {
        return (BrowserEvidenceStatus::Unavailable(reason), String::new());
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
    dirs.push(root.join(".anvil").join("evidence"));
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
            "not_enabled"
                | "adapter_not_implemented"
                | "unavailable"
                | "skipped"
                | "skipped_offline"
                | "skipped_unsupported_profile"
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
            let transition_observed =
                bool_field_deep(value, details, &["start_transition", "transition_observed"])
                    == Some(true)
                    || string_array_field_contains_deep(
                        value,
                        details,
                        "steps",
                        "start_transition",
                    )
                    || string_array_field_contains_deep(
                        value,
                        details,
                        "steps",
                        "recovery_transition",
                    );
            let startless_interaction_observed =
                surface_visible(value, details) && start_control_absent(value, details);
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
                if !transition_observed && !startless_interaction_observed {
                    return Some("start_transition_missing".to_string());
                }
                if bool_field_deep(value, details, &["input_state_evaluated_after_start"])
                    == Some(false)
                {
                    return Some("input_state_change_not_evaluated_after_start".to_string());
                }
                return Some("input_state_change_missing_after_start".to_string());
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
            let transition_observed =
                bool_field_deep(value, details, &["start_transition", "transition_observed"])
                    == Some(true)
                    || string_array_field_contains_deep(
                        value,
                        details,
                        "steps",
                        "start_transition",
                    )
                    || string_array_field_contains_deep(
                        value,
                        details,
                        "steps",
                        "recovery_transition",
                    );
            let input_state_changed = bool_field_deep(
                value,
                details,
                &[
                    "input_state_change",
                    "state_changed",
                    "visible_state_changed",
                ],
            ) == Some(true)
                || string_array_field_contains_deep(value, details, "steps", "input_state_change");
            input_state_changed
                && (transition_observed
                    || (surface_visible(value, details) && start_control_absent(value, details)))
        }
    }
}

fn surface_visible(value: &Value, details: Option<&Value>) -> bool {
    bool_field_deep(value, details, &["surface_visible", "interactive_surface"]) == Some(true)
        || string_array_field_contains_deep(value, details, "steps", "surface_visible")
}

fn start_control_absent(value: &Value, details: Option<&Value>) -> bool {
    bool_field_deep(
        value,
        details,
        &[
            "start_control_found",
            "start_control_present",
            "start_like_control_found",
            "start_like_control_present",
            "primary_action_found",
            "primary_action_present",
            "primary_control_found",
            "primary_control_present",
        ],
    ) == Some(false)
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
        if label == "browser_interaction" && interaction_probe_infrastructure_failure_reason(reason)
        {
            return;
        }
        missing_evidence.push(format!("{label}_failed:{reason}"));
    } else if let Some(reason) = status.strip_prefix("unavailable:") {
        if label == "browser_interaction" && interaction_probe_unavailable_reason_value(reason) {
            return;
        }
        inconclusive_reasons.push(format!("{label}_unavailable:{reason}"));
    }
}

fn interaction_probe_unavailable_reason(root: &Path) -> Option<String> {
    crate::minimal_loop::interaction_probe::playwright_availability(root)
        .unavailable_reason()
        .map(str::to_string)
}

fn interaction_probe_unavailable_reason_value(reason: &str) -> bool {
    matches!(reason, "playwright_not_installed" | "probe_unavailable")
}

fn interaction_probe_infrastructure_failure_reason(reason: &str) -> bool {
    reason.starts_with("probe_dependency_missing")
        || reason.starts_with("probe_infrastructure_failed")
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

fn string_array_field_contains_deep(
    value: &Value,
    details: Option<&Value>,
    key: &str,
    needle: &str,
) -> bool {
    string_array_field_contains(value, key, needle)
        || details.is_some_and(|details| string_array_field_contains(details, key, needle))
}

fn string_array_field_contains(value: &Value, key: &str, needle: &str) -> bool {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|item| item == needle)
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

#[allow(clippy::too_many_arguments)]
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
    verify_runtime_acceptance_with_browser_dirs_and_hints(
        root,
        required_paths,
        verify_commands,
        required_capabilities,
        required_evidence,
        required_obligations,
        deferred_verify_requirements,
        browser_evidence_dirs,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
pub fn verify_runtime_acceptance_with_hints(
    root: &Path,
    required_paths: &[String],
    verify_commands: &[String],
    required_capabilities: &[String],
    required_evidence: &[String],
    required_obligations: &[String],
    deferred_verify_requirements: &[String],
    evidence_hint_tokens: &[String],
) -> RuntimeAcceptanceReport {
    verify_runtime_acceptance_with_browser_dirs_and_hints(
        root,
        required_paths,
        verify_commands,
        required_capabilities,
        required_evidence,
        required_obligations,
        deferred_verify_requirements,
        &[],
        evidence_hint_tokens,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn verify_runtime_acceptance_with_browser_dirs_and_hints(
    root: &Path,
    required_paths: &[String],
    verify_commands: &[String],
    required_capabilities: &[String],
    required_evidence: &[String],
    required_obligations: &[String],
    deferred_verify_requirements: &[String],
    browser_evidence_dirs: &[PathBuf],
    evidence_hint_tokens: &[String],
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
    let artifact_obligations = artifact_obligation_evidence_with_hints(
        root,
        required_paths,
        required_capabilities,
        evidence_hint_tokens,
    );
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
    let mut diagnostics = Vec::new();
    let mut evidence_tiers = BTreeMap::new();
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
                record_bool_evidence_tier(
                    evidence,
                    has_implementation_artifact(root, required_paths, &workspace),
                    &mut missing_evidence,
                    &mut evidence_tiers,
                );
            }
            "test_artifact" => {
                record_bool_evidence_tier(
                    evidence,
                    has_test_artifact(&workspace),
                    &mut missing_evidence,
                    &mut evidence_tiers,
                );
            }
            "bound_verify_command" => {
                record_bool_evidence_tier(
                    evidence,
                    has_bound_verify_command(verify_commands, &workspace),
                    &mut missing_evidence,
                    &mut evidence_tiers,
                );
            }
            "non_zero_test_or_assertion_evidence" => {
                record_bool_evidence_tier(
                    evidence,
                    has_assertion_or_test_evidence(&workspace),
                    &mut missing_evidence,
                    &mut evidence_tiers,
                );
            }
            "build_command_or_dependency_missing_boundary" => {
                record_bool_evidence_tier(
                    evidence,
                    has_build_command_or_dependency_boundary(
                        verify_commands,
                        deferred_verify_requirements,
                        &workspace,
                    ),
                    &mut missing_evidence,
                    &mut evidence_tiers,
                );
            }
            "interactive_ui_source_evidence" => {
                record_bool_evidence_tier(
                    evidence,
                    has_interactive_ui_source(&workspace),
                    &mut missing_evidence,
                    &mut evidence_tiers,
                );
            }
            "non_static_screen_evidence" => {
                record_bool_evidence_tier(
                    evidence,
                    has_non_static_screen_evidence(&workspace),
                    &mut missing_evidence,
                    &mut evidence_tiers,
                );
            }
            "visible_interactive_surface_evidence" => {
                record_bool_evidence_tier(
                    evidence,
                    has_visible_interactive_surface_evidence(&workspace),
                    &mut missing_evidence,
                    &mut evidence_tiers,
                );
            }
            "user_input_handler_evidence" => {
                record_source_signal(
                    evidence,
                    user_input_handler_signal(&workspace),
                    &mut missing_evidence,
                    &mut weak_evidence,
                    &mut evidence_tiers,
                );
            }
            "stateful_update_evidence" => {
                record_bool_evidence_tier(
                    evidence,
                    has_stateful_update_evidence(&workspace),
                    &mut missing_evidence,
                    &mut evidence_tiers,
                );
            }
            "challenge_or_adversary_evidence" => {
                record_bool_evidence_tier(
                    evidence,
                    has_challenge_or_adversary_evidence(&workspace, evidence_hint_tokens),
                    &mut missing_evidence,
                    &mut evidence_tiers,
                );
            }
            "score_or_progression_evidence" => {
                record_source_signal(
                    evidence,
                    score_or_progression_signal(&workspace),
                    &mut missing_evidence,
                    &mut weak_evidence,
                    &mut evidence_tiers,
                );
            }
            "failure_or_collision_evidence" => {
                record_source_signal(
                    evidence,
                    failure_or_collision_signal(&workspace),
                    &mut missing_evidence,
                    &mut weak_evidence,
                    &mut evidence_tiers,
                );
            }
            "restart_or_recoverable_state_evidence" => {
                record_source_signal(
                    evidence,
                    restart_or_recoverable_state_signal(&workspace),
                    &mut missing_evidence,
                    &mut weak_evidence,
                    &mut evidence_tiers,
                );
            }
            "persistence_evidence" => {
                record_bool_evidence_tier(
                    evidence,
                    has_persistence_evidence(&workspace),
                    &mut missing_evidence,
                    &mut evidence_tiers,
                );
            }
            "nextjs_route_evidence" => {
                record_bool_evidence_tier(
                    evidence,
                    has_nextjs_route_evidence(&workspace),
                    &mut missing_evidence,
                    &mut evidence_tiers,
                );
            }
            "requested_content_evidence" => {
                record_bool_evidence_tier(
                    evidence,
                    workspace.readme || required_paths.iter().any(|path| path.ends_with(".md")),
                    &mut missing_evidence,
                    &mut evidence_tiers,
                );
            }
            unknown => {
                missing_evidence.push(format!("unsupported_required_evidence:{unknown}"));
                evidence_tiers.insert(
                    unknown.to_string(),
                    EvidenceTier::Absent.as_str().to_string(),
                );
            }
        }
    }

    collect_weak_verify_evidence(verify_commands, &workspace, &mut weak_evidence);
    collect_route_unbound_capability_evidence(
        &workspace,
        &required,
        &missing_evidence,
        evidence_hint_tokens,
        &mut weak_evidence,
        &mut diagnostics,
    );
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
        diagnostics,
        unverified_evidence: Vec::new(),
        evidence_tiers,
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

pub(crate) fn refresh_runtime_acceptance_report(
    report: &mut RuntimeAcceptanceReport,
    required_capabilities: &[String],
    required_evidence: &[String],
    required_obligations: &[String],
) {
    report.inconclusive = !report.inconclusive_reasons.is_empty();
    report.capability_evidence_bindings = capability_evidence_bindings(
        required_capabilities,
        &report.artifact_obligations,
        &report.missing_evidence,
    );
    let weak_evidence_blocks_completion = !report.weak_evidence.is_empty()
        && source_first_completion_authority_required(
            required_capabilities,
            required_evidence,
            required_obligations,
        );
    report.passed = report.missing_capabilities.is_empty()
        && report.missing_evidence.is_empty()
        && report.missing_obligations.is_empty()
        && !report.inconclusive
        && !weak_evidence_blocks_completion;
    report.primary_reason = if let Some(reason) = report.missing_capabilities.first() {
        format!("missing_required_capabilities:{reason}")
    } else if let Some(reason) = report.missing_evidence.first() {
        format!("missing_required_evidence:{reason}")
    } else if let Some(reason) = report.missing_obligations.first() {
        format!("missing_required_obligations:{reason}")
    } else if let Some(reason) = report.inconclusive_reasons.first() {
        format!("inconclusive_acceptance:{reason}")
    } else if let Some(reason) = report.weak_evidence.first() {
        format!("weak_verification_evidence:{reason}")
    } else {
        "pass".to_string()
    };
}

pub fn artifact_obligation_evidence(
    root: &Path,
    required_paths: &[String],
    required_capabilities: &[String],
) -> Vec<ArtifactObligationEvidence> {
    artifact_obligation_evidence_with_hints(root, required_paths, required_capabilities, &[])
}

pub fn artifact_obligation_evidence_with_hints(
    root: &Path,
    required_paths: &[String],
    required_capabilities: &[String],
    evidence_hint_tokens: &[String],
) -> Vec<ArtifactObligationEvidence> {
    let mut out = Vec::new();
    let route_bound_files = route_bound_closure(root, "nextjs");
    for path in required_paths {
        let full = root.join(path);
        if !full.is_file() {
            continue;
        }
        let content = std::fs::read_to_string(&full).unwrap_or_default();
        let route_bound = route_bound_files.contains(Path::new(path));
        let file = SourceFile::new_with_route_bound(path.clone(), content, route_bound);
        let role = artifact_role_for_file(&file);
        let evidence = evidence_kinds_for_file(&file, evidence_hint_tokens)
            .into_iter()
            .map(|kind| kind.as_str().to_string())
            .collect::<BTreeSet<_>>();
        let required_capabilities_supported = required_capabilities
            .iter()
            .filter(|capability| {
                route_bound
                    && evidence_kinds_for_capability(capability)
                        .into_iter()
                        .any(|kind| evidence.contains(kind.as_str()))
            })
            .cloned()
            .collect::<Vec<_>>();
        out.push(ArtifactObligationEvidence {
            path: path.clone(),
            role: role.as_str().to_string(),
            evidence: evidence.into_iter().collect(),
            route_bound,
            satisfies_implementation: route_bound && role.satisfies_implementation(),
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
                    || (artifact.route_bound
                        && artifact
                            .evidence
                            .iter()
                            .any(|evidence| required_set.contains(evidence)))
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
        "persistence" => vec![
            EvidenceKind::ImplementationArtifact,
            EvidenceKind::StatefulUpdateEvidence,
            EvidenceKind::PersistenceEvidence,
        ],
        "nextjs_route" | "route" => vec![EvidenceKind::NextJsRouteEvidence],
        _ => Vec::new(),
    }
}

fn collect_workspace_evidence(root: &Path) -> WorkspaceEvidence {
    let mut evidence = WorkspaceEvidence::default();
    let route_bound_files = route_bound_closure(root, "nextjs");
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
        let file = SourceFile::new_with_route_bound(
            rel.clone(),
            content,
            route_bound_files.contains(Path::new(&rel)),
        );
        if looks_like_test_file(&rel) {
            evidence.test_files.push(file.clone());
        }
        evidence.source_files.push(file);
    }
    evidence
}

fn route_bound_source_files(workspace: &WorkspaceEvidence) -> impl Iterator<Item = &SourceFile> {
    workspace
        .source_files
        .iter()
        .filter(|file| file.route_bound)
}

fn route_unbound_source_files(workspace: &WorkspaceEvidence) -> impl Iterator<Item = &SourceFile> {
    workspace
        .source_files
        .iter()
        .filter(|file| !file.route_bound && is_route_importable_source_path(&file.rel))
}

fn is_route_importable_source_path(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|ext| {
        matches!(
            ext.to_string_lossy().to_ascii_lowercase().as_str(),
            "tsx" | "ts" | "jsx" | "js" | "css"
        )
    })
}

pub fn comment_stripped_source_corpus(root: &Path) -> String {
    collect_workspace_evidence(root)
        .source_files
        .into_iter()
        .map(|file| file.comments_stripped_strings_preserved)
        .collect::<Vec<_>>()
        .join("\n")
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
        workspace
            .source_files
            .iter()
            .find(|file| file.rel == *path)
            .is_some_and(|file| {
                file.route_bound && artifact_role_for_file(file).satisfies_implementation()
            })
            || route_bound_closure(root, "nextjs").contains(Path::new(path))
                && root.join(path).is_file()
                && artifact_role_for_file(&SourceFile::new(
                    path.clone(),
                    std::fs::read_to_string(root.join(path)).unwrap_or_default(),
                ))
                .satisfies_implementation()
    }) || route_bound_source_files(workspace)
        .any(|file| artifact_role_for_file(file).satisfies_implementation())
}

fn has_test_artifact(workspace: &WorkspaceEvidence) -> bool {
    !workspace.test_files.is_empty()
        || workspace
            .source_files
            .iter()
            .any(has_inline_test_or_self_test)
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

fn collect_route_unbound_capability_evidence(
    workspace: &WorkspaceEvidence,
    required_evidence: &BTreeSet<String>,
    missing_evidence: &[String],
    evidence_hint_tokens: &[String],
    weak: &mut Vec<String>,
    diagnostics: &mut Vec<String>,
) {
    let missing = missing_evidence.iter().cloned().collect::<BTreeSet<_>>();
    for file in route_unbound_source_files(workspace) {
        let route_scanned_kinds = evidence_kinds_for_file(file, evidence_hint_tokens)
            .into_iter()
            .filter(|kind| source_scanned_evidence_kind(*kind))
            .collect::<Vec<_>>();
        if route_scanned_kinds.is_empty() {
            continue;
        }
        weak.push(format!("route_unbound:{}", file.rel));
        if route_scanned_kinds.iter().any(|kind| {
            let key = kind.as_str();
            required_evidence.contains(key) && missing.contains(key)
        }) {
            diagnostics.push(format!("route_unbound_capability_artifact:{}", file.rel));
        }
    }
    weak.sort();
    weak.dedup();
    diagnostics.sort();
    diagnostics.dedup();
}

fn source_scanned_evidence_kind(kind: EvidenceKind) -> bool {
    matches!(
        kind,
        EvidenceKind::InteractiveUiSourceEvidence
            | EvidenceKind::NonStaticScreenEvidence
            | EvidenceKind::VisibleInteractiveSurfaceEvidence
            | EvidenceKind::UserInputHandlerEvidence
            | EvidenceKind::StatefulUpdateEvidence
            | EvidenceKind::ChallengeOrAdversaryEvidence
            | EvidenceKind::ScoreOrProgressionEvidence
            | EvidenceKind::FailureOrCollisionEvidence
            | EvidenceKind::RestartOrRecoverableStateEvidence
            | EvidenceKind::PersistenceEvidence
            | EvidenceKind::NextJsRouteEvidence
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceEvidenceSignal {
    Absent,
    Weak(&'static str),
    Strong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvidenceTier {
    Strong,
    Weak,
    Absent,
}

impl EvidenceTier {
    fn as_str(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Weak => "weak",
            Self::Absent => "absent",
        }
    }
}

fn record_bool_evidence_tier(
    evidence: &str,
    present: bool,
    missing_evidence: &mut Vec<String>,
    evidence_tiers: &mut BTreeMap<String, String>,
) {
    if present {
        evidence_tiers.insert(
            evidence.to_string(),
            EvidenceTier::Strong.as_str().to_string(),
        );
    } else {
        missing_evidence.push(evidence.to_string());
        evidence_tiers.insert(
            evidence.to_string(),
            EvidenceTier::Absent.as_str().to_string(),
        );
    }
}

fn record_source_signal(
    evidence: &str,
    signal: SourceEvidenceSignal,
    missing_evidence: &mut Vec<String>,
    weak_evidence: &mut Vec<String>,
    evidence_tiers: &mut BTreeMap<String, String>,
) {
    match signal {
        SourceEvidenceSignal::Strong => {
            evidence_tiers.insert(
                evidence.to_string(),
                EvidenceTier::Strong.as_str().to_string(),
            );
        }
        SourceEvidenceSignal::Weak(reason) => {
            missing_evidence.push(evidence.to_string());
            weak_evidence.push(format!("weak_source_evidence:{evidence}:{reason}"));
            evidence_tiers.insert(
                evidence.to_string(),
                EvidenceTier::Weak.as_str().to_string(),
            );
        }
        SourceEvidenceSignal::Absent => {
            missing_evidence.push(evidence.to_string());
            evidence_tiers.insert(
                evidence.to_string(),
                EvidenceTier::Absent.as_str().to_string(),
            );
        }
    }
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
    route_bound_source_files(workspace).any(source_file_has_interactive_ui)
}

fn has_non_static_screen_evidence(workspace: &WorkspaceEvidence) -> bool {
    route_bound_source_files(workspace).any(source_file_has_non_static_screen)
}

fn has_visible_interactive_surface_evidence(workspace: &WorkspaceEvidence) -> bool {
    route_bound_source_files(workspace).any(source_file_has_visible_interactive_surface)
}

fn user_input_handler_signal(workspace: &WorkspaceEvidence) -> SourceEvidenceSignal {
    workspace_source_signal(workspace, source_file_user_input_handler_signal)
}

fn has_stateful_update_evidence(workspace: &WorkspaceEvidence) -> bool {
    route_bound_source_files(workspace).any(source_file_has_stateful_update)
}

fn has_challenge_or_adversary_evidence(
    workspace: &WorkspaceEvidence,
    evidence_hint_tokens: &[String],
) -> bool {
    route_bound_source_files(workspace)
        .any(|file| source_file_has_challenge_or_adversary(file, evidence_hint_tokens))
}

fn score_or_progression_signal(workspace: &WorkspaceEvidence) -> SourceEvidenceSignal {
    workspace_source_signal(workspace, source_file_score_or_progression_signal)
}

fn failure_or_collision_signal(workspace: &WorkspaceEvidence) -> SourceEvidenceSignal {
    workspace_source_signal(workspace, source_file_failure_or_collision_signal)
}

fn restart_or_recoverable_state_signal(workspace: &WorkspaceEvidence) -> SourceEvidenceSignal {
    workspace_source_signal(workspace, source_file_restart_or_recoverable_state_signal)
}

fn has_persistence_evidence(workspace: &WorkspaceEvidence) -> bool {
    route_bound_source_files(workspace).any(source_file_has_persistence)
}

fn workspace_source_signal(
    workspace: &WorkspaceEvidence,
    signal_fn: fn(&SourceFile) -> SourceEvidenceSignal,
) -> SourceEvidenceSignal {
    let mut found_weak = SourceEvidenceSignal::Absent;
    for file in route_bound_source_files(workspace) {
        match signal_fn(file) {
            SourceEvidenceSignal::Strong => return SourceEvidenceSignal::Strong,
            SourceEvidenceSignal::Weak(reason) => found_weak = SourceEvidenceSignal::Weak(reason),
            SourceEvidenceSignal::Absent => {}
        }
    }
    found_weak
}

fn has_nextjs_route_evidence(workspace: &WorkspaceEvidence) -> bool {
    route_bound_source_files(workspace).any(|file| {
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

fn evidence_kinds_for_file(
    file: &SourceFile,
    evidence_hint_tokens: &[String],
) -> Vec<EvidenceKind> {
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
    if matches!(
        source_file_user_input_handler_signal(file),
        SourceEvidenceSignal::Strong
    ) {
        kinds.push(EvidenceKind::UserInputHandlerEvidence);
    }
    if source_file_has_stateful_update(file) {
        kinds.push(EvidenceKind::StatefulUpdateEvidence);
    }
    if source_file_has_challenge_or_adversary(file, evidence_hint_tokens) {
        kinds.push(EvidenceKind::ChallengeOrAdversaryEvidence);
    }
    if matches!(
        source_file_score_or_progression_signal(file),
        SourceEvidenceSignal::Strong
    ) {
        kinds.push(EvidenceKind::ScoreOrProgressionEvidence);
    }
    if matches!(
        source_file_failure_or_collision_signal(file),
        SourceEvidenceSignal::Strong
    ) {
        kinds.push(EvidenceKind::FailureOrCollisionEvidence);
    }
    if matches!(
        source_file_restart_or_recoverable_state_signal(file),
        SourceEvidenceSignal::Strong
    ) {
        kinds.push(EvidenceKind::RestartOrRecoverableStateEvidence);
    }
    if source_file_has_persistence(file) {
        kinds.push(EvidenceKind::PersistenceEvidence);
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
                || workspace.source_files.iter().any(|file| {
                    file.route_bound && artifact_role_for_file(file) == ArtifactRoleLite::Scaffold
                })
        }
        "implementation" => {
            artifact_obligations
                .iter()
                .any(|obligation| obligation.satisfies_implementation)
                || route_bound_source_files(workspace)
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
    let content = file.scan_text();
    let lower = content.to_ascii_lowercase();
    (content.contains("useState")
        || content.contains("useReducer")
        || content.contains("addEventListener")
        || content.contains("onKeyDown")
        || content.contains("onClick")
        || lower.contains("onchange")
        || lower.contains("onsubmit")
        || content.contains("requestAnimationFrame")
        || lower.contains("<canvas"))
        && (lower.contains("keydown")
            || lower.contains("arrow")
            || lower.contains("click")
            || lower.contains("change")
            || lower.contains("submit")
            || lower.contains("<input")
            || lower.contains("<textarea")
            || lower.contains("<select")
            || lower.contains("pointer")
            || lower.contains("touch")
            || lower.contains("canvas"))
}

fn source_file_has_non_static_screen(file: &SourceFile) -> bool {
    let lower = file.scan_text().to_ascii_lowercase();
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
    let lower = file.scan_text().to_ascii_lowercase();
    let strings_preserved = file
        .comments_stripped_strings_preserved_text()
        .to_ascii_lowercase();
    lower.contains("<canvas")
        || lower.contains("<button")
        || lower.contains("<input")
        || lower.contains("<select")
        || lower.contains("<textarea")
        || lower.contains("onclick")
        || lower.contains("onkeydown")
        || lower.contains("onpointer")
        || strings_preserved.contains("role=\"button\"")
        || strings_preserved.contains("role='button'")
        || lower.contains("tabindex")
}

fn source_file_has_user_input_handler_keyword(file: &SourceFile) -> bool {
    let content = file.scan_text();
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
    let content = file.scan_text();
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

fn source_file_has_challenge_or_adversary(
    file: &SourceFile,
    evidence_hint_tokens: &[String],
) -> bool {
    source_file_has_static_adversary_entity(file)
        || (source_file_has_goal_adversary_hint(file, evidence_hint_tokens)
            && source_file_has_position_or_motion_update(file)
            && source_file_has_adversary_motion_or_interaction_signal(file))
}

fn source_file_has_static_adversary_entity(file: &SourceFile) -> bool {
    let lower = file.scan_text().to_ascii_lowercase();
    let has_adversary_token = [
        "enemy",
        "enemies",
        "adversary",
        "opponent",
        "obstacle",
        "hazard",
        "invader",
        "alien",
        "ufo",
        "asteroid",
        "monster",
        "zombie",
        "mob",
        "wave",
        "spawn",
        "target",
        "challenge",
        "boss",
        "敵",
        "インベーダー",
        "エイリアン",
        "モンスター",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    if !has_adversary_token {
        return false;
    }
    source_text_has_adversary_entity_context(&lower)
}

fn source_text_has_adversary_entity_context(lower: &str) -> bool {
    [
        "x:",
        "y:",
        ".x",
        ".y",
        "array.from",
        ".map(",
        ".foreach(",
        ".filter(",
        "setenemies(",
        "setinvaders(",
        "enemy =",
        "enemy=",
        "enemies =",
        "enemies=",
        "invader =",
        "invader=",
        "invaders =",
        "invaders=",
        "const enemy",
        "const enemies",
        "const invader",
        "const invaders",
        "let enemy",
        "let enemies",
        "let invader",
        "let invaders",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn source_file_has_goal_adversary_hint(file: &SourceFile, evidence_hint_tokens: &[String]) -> bool {
    let lower = file.scan_text().to_ascii_lowercase();
    evidence_hint_tokens
        .iter()
        .map(|token| token.trim())
        .filter(|token| !token.is_empty())
        .any(|token| {
            if token.is_ascii() {
                lower.contains(&token.to_ascii_lowercase())
            } else {
                file.scan_text().contains(token)
            }
        })
}

fn source_file_has_adversary_motion_or_interaction_signal(file: &SourceFile) -> bool {
    source_file_has_stateful_update(file)
        || source_file_has_failure_or_collision(file)
        || source_file_has_position_or_motion_update(file)
}

fn source_file_has_position_or_motion_update(file: &SourceFile) -> bool {
    let lower = file.scan_text().to_ascii_lowercase();
    let has_position_or_motion_token = [
        "position",
        "positions",
        "velocity",
        "speed",
        "move",
        "movement",
        "direction",
        ".x",
        ".y",
        "x:",
        "y:",
        "left",
        "top",
        "translate",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let has_update_token = [
        "+=",
        "-=",
        "map(",
        "filter(",
        "set",
        "update",
        "tick",
        "frame",
        "requestanimationframe",
        "setinterval",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    has_position_or_motion_token && has_update_token
}

fn source_file_has_score_or_progression_keyword(file: &SourceFile) -> bool {
    let lower = file.scan_text().to_ascii_lowercase();
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
    matches!(
        source_file_failure_or_collision_signal(file),
        SourceEvidenceSignal::Strong
    )
}

fn source_file_has_failure_or_collision_keyword(file: &SourceFile) -> bool {
    let lower = file.scan_text().to_ascii_lowercase();
    [
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
    .any(|needle| lower.contains(needle))
}

fn source_file_has_restart_or_recoverable_state_keyword(file: &SourceFile) -> bool {
    let lower = file.scan_text().to_ascii_lowercase();
    [
        "start",
        "restart",
        "reset",
        "pause",
        "resume",
        "gameover",
        "game over",
        "play again",
        "try again",
        "initgame",
        "initstate",
        "resetstate",
        "newgame",
        "newstate",
        "newlevel",
        "スタート",
        "開始",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn source_file_has_persistence(file: &SourceFile) -> bool {
    let lower = file.scan_text().to_ascii_lowercase();
    [
        "localstorage",
        "sessionstorage",
        "indexeddb",
        ".setitem(",
        ".getitem(",
        "navigator.storage",
        "caches.open(",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn source_file_user_input_handler_signal(file: &SourceFile) -> SourceEvidenceSignal {
    if !source_file_has_user_input_handler_keyword(file) {
        return SourceEvidenceSignal::Absent;
    }
    if source_file_has_mutating_input_handler(file) {
        SourceEvidenceSignal::Strong
    } else {
        SourceEvidenceSignal::Weak(
            "input handler must mutate state or call a non-audio gameplay function",
        )
    }
}

fn source_file_score_or_progression_signal(file: &SourceFile) -> SourceEvidenceSignal {
    if !source_file_has_score_or_progression_keyword(file) {
        return SourceEvidenceSignal::Absent;
    }
    if source_file_has_score_update_signal(file) {
        SourceEvidenceSignal::Strong
    } else {
        SourceEvidenceSignal::Weak(
            "score/progression needs an executable update such as score +=, score++, score =, or setScore(...)",
        )
    }
}

fn source_file_failure_or_collision_signal(file: &SourceFile) -> SourceEvidenceSignal {
    if !source_file_has_failure_or_collision_keyword(file) {
        return SourceEvidenceSignal::Absent;
    }
    if source_file_has_game_over_transition(file) || source_file_has_collision_conditional(file) {
        SourceEvidenceSignal::Strong
    } else {
        SourceEvidenceSignal::Weak(
            "failure/collision needs a game-over transition or conditional overlap/intersect/distance comparison",
        )
    }
}

fn source_file_restart_or_recoverable_state_signal(file: &SourceFile) -> SourceEvidenceSignal {
    if !source_file_has_restart_or_recoverable_state_keyword(file) {
        return SourceEvidenceSignal::Absent;
    }
    if source_file_has_restart_reset_handler(file) {
        SourceEvidenceSignal::Strong
    } else {
        SourceEvidenceSignal::Weak(
            "wire a handler (e.g. onClick) to a function that resets score AND entities and transitions out of the game-over state",
        )
    }
}

fn source_file_has_mutating_input_handler(file: &SourceFile) -> bool {
    let lower = file.scan_text().to_ascii_lowercase();
    handler_segments(&lower).into_iter().any(|segment| {
        segment_has_state_mutation(segment) || segment_has_non_audio_gameplay_call(segment)
    })
}

fn handler_segments(lower: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    for needle in [
        "restart",
        "resetgame",
        "restartgame",
        "initgame",
        "initstate",
        "resetstate",
        "startgame",
        "newgame",
        "onkeydown",
        "onkeyup",
        "onclick",
        "onpointer",
        "onmousedown",
        "onmouseup",
        "ontouch",
        "onsubmit",
        "onchange",
        "pointerdown",
        "touchstart",
        "keydown",
        "keyup",
        "addeventlistener",
    ] {
        for (index, _) in lower.match_indices(needle) {
            let end = lower.len().min(index + 500);
            segments.push(&lower[index..end]);
        }
    }
    segments
}

fn segment_has_state_mutation(segment: &str) -> bool {
    [
        "setscore(",
        "setpoints(",
        "setlevel(",
        "setstage(",
        "setwave(",
        "setcombo(",
        "setprogress(",
        "setbullets(",
        "setbullet(",
        "setshots(",
        "setprojectiles(",
        "setenemies(",
        "setinvaders(",
        "setplayer(",
        "setgamestate(",
        "setgameover(",
        "dispatch(",
        ".push(",
        ".splice(",
        "+=",
        "-=",
        "++",
        "--",
        "=> set",
        "=>set",
    ]
    .iter()
    .any(|needle| segment.contains(needle))
}

fn segment_has_non_audio_gameplay_call(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if !is_identifier_start(bytes[index]) {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < bytes.len() && is_identifier_continue(bytes[index]) {
            index += 1;
        }
        let ident = &segment[start..index];
        let mut lookahead = index;
        while lookahead < bytes.len() && bytes[lookahead].is_ascii_whitespace() {
            lookahead += 1;
        }
        if lookahead < bytes.len()
            && bytes[lookahead] == b'('
            && non_audio_gameplay_call_name(ident)
        {
            return true;
        }
    }
    false
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$')
}

fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

fn non_audio_gameplay_call_name(name: &str) -> bool {
    if matches!(
        name,
        "if" | "for"
            | "while"
            | "switch"
            | "catch"
            | "function"
            | "useeffect"
            | "map"
            | "foreach"
            | "some"
            | "filter"
            | "addeventlistener"
            | "removeeventlistener"
            | "preventdefault"
            | "stoppropagation"
            | "settimeout"
            | "setinterval"
            | "requestanimationframe"
            | "play"
            | "pause"
    ) || ["audio", "sound", "music", "beep", "tone"]
        .iter()
        .any(|needle| name.contains(needle))
    {
        return false;
    }
    [
        "shoot",
        "fire",
        "bullet",
        "projectile",
        "move",
        "jump",
        "start",
        "restart",
        "reset",
        "spawn",
        "launch",
        "attack",
        "defend",
        "select",
        "submit",
        "toggle",
        "advance",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

fn source_file_has_score_update_signal(file: &SourceFile) -> bool {
    let lower = file.scan_text().to_ascii_lowercase();
    [
        "setscore(",
        "setpoints(",
        "setlevel(",
        "setstage(",
        "setwave(",
        "setcombo(",
        "setprogress(",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || [
            "score", "points", "level", "stage", "wave", "combo", "progress",
        ]
        .iter()
        .any(|name| identifier_has_assignment_or_increment(&lower, name))
}

fn identifier_has_assignment_or_increment(lower: &str, name: &str) -> bool {
    [
        format!("{name} +="),
        format!("{name}+="),
        format!("{name}++"),
        format!("++{name}"),
        format!("{name} ="),
        format!("{name}="),
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn source_file_has_game_over_transition(file: &SourceFile) -> bool {
    let lower = file.scan_text().to_ascii_lowercase();
    let strings_preserved = file
        .comments_stripped_strings_preserved_text()
        .to_ascii_lowercase();
    [
        "setgameover(",
        "gameover = true",
        "gameover=true",
        "isgameover = true",
        "isgameover=true",
        "setgamestate(gameover",
        "setstatus(gameover",
        "setscreen(gameover",
        "setmode(gameover",
        "dispatch({type:gameover",
        "dispatch({ type: gameover",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || [
            "setgamestate(\"gameover\"",
            "setgamestate('gameover'",
            "setgamestate(`gameover`",
            "setgamestate(\"game over\"",
            "setgamestate('game over'",
            "setstatus(\"gameover\"",
            "setstatus('gameover'",
            "setscreen(\"gameover\"",
            "setscreen('gameover'",
            "setmode(\"gameover\"",
            "setmode('gameover'",
        ]
        .iter()
        .any(|needle| strings_preserved.contains(needle))
}

fn source_file_has_collision_conditional(file: &SourceFile) -> bool {
    let lower = file.scan_text().to_ascii_lowercase();
    lower
        .match_indices("if")
        .filter_map(|(index, _)| if_condition_segment(&lower, index))
        .any(|segment| {
            [
                "overlap",
                "intersect",
                "collision",
                "collide",
                "distance",
                "math.abs",
                ".x",
                ".y",
            ]
            .iter()
            .any(|needle| segment.contains(needle))
                && ["<=", ">=", "<", ">"].iter().any(|op| segment.contains(op))
        })
}

fn if_condition_segment(lower: &str, if_index: usize) -> Option<&str> {
    let bytes = lower.as_bytes();
    let mut index = if_index + 2;
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    if bytes.get(index) != Some(&b'(') {
        return None;
    }
    let start = index;
    let mut depth = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return lower.get(start..=index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn source_file_has_restart_reset_handler(file: &SourceFile) -> bool {
    let lower = file.scan_text().to_ascii_lowercase();
    let strings_preserved = file
        .comments_stripped_strings_preserved_text()
        .to_ascii_lowercase();
    let has_game_over_reference = source_text_has_game_over_reference(&lower, &strings_preserved);
    if has_game_over_reference && source_text_has_single_segment_restart_reset_handler(&lower) {
        return true;
    }
    source_text_has_linked_restart_reset_function(
        &lower,
        &strings_preserved,
        has_game_over_reference,
    )
}

fn source_text_has_game_over_reference(lower: &str, strings_preserved: &str) -> bool {
    [
        "gameover",
        "game over",
        "isgameover",
        "setgameover(",
        "setgamestate(\"gameover\"",
        "setgamestate('gameover'",
    ]
    .iter()
    .any(|needle| lower.contains(needle) || strings_preserved.contains(needle))
}

fn source_text_has_single_segment_restart_reset_handler(lower: &str) -> bool {
    handler_segments(lower).into_iter().any(|segment| {
        restart_segment_resets_score(segment)
            && restart_segment_resets_entities(segment)
            && restart_segment_references_recoverable_state(segment)
    })
}

fn source_text_has_linked_restart_reset_function(
    lower: &str,
    strings_preserved: &str,
    has_game_over_reference: bool,
) -> bool {
    restart_reset_function_candidates(lower, strings_preserved)
        .into_iter()
        .filter(|candidate| restart_segment_resets_score(&candidate.body))
        .filter(|candidate| restart_segment_resets_entities(&candidate.body))
        .any(|candidate| {
            restart_function_is_referenced_from_handler(lower, &candidate.name)
                || (restart_intent_function_name(&candidate.name) && has_game_over_reference)
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RestartResetFunctionCandidate {
    name: String,
    body: String,
}

fn restart_reset_function_candidates(
    lower: &str,
    strings_preserved: &str,
) -> Vec<RestartResetFunctionCandidate> {
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    collect_function_declaration_candidates(lower, strings_preserved, &mut candidates, &mut seen);
    collect_arrow_assignment_candidates(lower, strings_preserved, &mut candidates, &mut seen);
    candidates
}

fn collect_function_declaration_candidates(
    lower: &str,
    strings_preserved: &str,
    candidates: &mut Vec<RestartResetFunctionCandidate>,
    seen: &mut BTreeSet<(String, usize)>,
) {
    let mut search_start = 0usize;
    while let Some(relative) = lower[search_start..].find("function") {
        let index = search_start + relative;
        let after_keyword = index + "function".len();
        if !keyword_boundary(lower, index, after_keyword) {
            search_start = after_keyword;
            continue;
        }
        let mut cursor = skip_ascii_whitespace(lower, after_keyword);
        let Some((name, name_end)) = read_identifier(lower, cursor) else {
            search_start = after_keyword;
            continue;
        };
        cursor = skip_ascii_whitespace(lower, name_end);
        if lower.as_bytes().get(cursor) != Some(&b'(') {
            search_start = after_keyword;
            continue;
        }
        let body_search_start = find_matching_delimiter(lower, cursor, b'(', b')')
            .map(|end| end + 1)
            .unwrap_or(cursor + 1);
        push_function_candidate(
            name,
            lower,
            strings_preserved,
            body_search_start,
            candidates,
            seen,
        );
        search_start = after_keyword;
    }
}

fn collect_arrow_assignment_candidates(
    lower: &str,
    strings_preserved: &str,
    candidates: &mut Vec<RestartResetFunctionCandidate>,
    seen: &mut BTreeSet<(String, usize)>,
) {
    let mut cursor = 0usize;
    while cursor < lower.len() {
        let byte = lower.as_bytes()[cursor];
        if !is_identifier_start(byte) {
            cursor += 1;
            continue;
        }
        let Some((name, name_end)) = read_identifier(lower, cursor) else {
            cursor += 1;
            continue;
        };
        if matches!(name, "const" | "let" | "var" | "function" | "return") {
            cursor = name_end;
            continue;
        }
        let mut after_name = skip_ascii_whitespace(lower, name_end);
        if lower.as_bytes().get(after_name) != Some(&b'=') {
            cursor = name_end;
            continue;
        }
        after_name = skip_ascii_whitespace(lower, after_name + 1);
        if lower[after_name..].strip_prefix("async").is_some() {
            let async_end = after_name + "async".len();
            if keyword_boundary(lower, after_name, async_end) {
                after_name = skip_ascii_whitespace(lower, async_end);
            }
        }
        let Some(body_search_start) = arrow_function_body_search_start(lower, after_name) else {
            cursor = name_end;
            continue;
        };
        push_function_candidate(
            name,
            lower,
            strings_preserved,
            body_search_start,
            candidates,
            seen,
        );
        cursor = name_end;
    }
}

fn arrow_function_body_search_start(lower: &str, start: usize) -> Option<usize> {
    let bytes = lower.as_bytes();
    let arrow_search_start = if bytes.get(start) == Some(&b'(') {
        find_matching_delimiter(lower, start, b'(', b')')? + 1
    } else {
        let (_, ident_end) = read_identifier(lower, start)?;
        ident_end
    };
    let after_params = skip_ascii_whitespace(lower, arrow_search_start);
    lower[after_params..]
        .find("=>")
        .map(|relative| after_params + relative + "=>".len())
}

fn push_function_candidate(
    name: &str,
    lower: &str,
    strings_preserved: &str,
    body_search_start: usize,
    candidates: &mut Vec<RestartResetFunctionCandidate>,
    seen: &mut BTreeSet<(String, usize)>,
) {
    let Some((body_start, body_end)) = function_body_span(lower, body_search_start) else {
        return;
    };
    let name = name.to_string();
    if !seen.insert((name.clone(), body_start)) {
        return;
    }
    let body = strings_preserved
        .get(body_start..body_end)
        .or_else(|| lower.get(body_start..body_end))
        .unwrap_or_default()
        .to_string();
    candidates.push(RestartResetFunctionCandidate { name, body });
}

fn function_body_span(lower: &str, search_start: usize) -> Option<(usize, usize)> {
    let body_start = lower[search_start..].find('{')? + search_start;
    let body_end = find_matching_delimiter(lower, body_start, b'{', b'}')
        .map(|end| end + 1)
        .unwrap_or_else(|| lower.len().min(body_start + 1500));
    Some((body_start, body_end))
}

fn find_matching_delimiter(text: &str, start: usize, open: u8, close: u8) -> Option<usize> {
    if text.as_bytes().get(start) != Some(&open) {
        return None;
    }
    let mut depth = 0usize;
    for (index, byte) in text.as_bytes().iter().enumerate().skip(start) {
        if *byte == open {
            depth += 1;
        } else if *byte == close {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

fn skip_ascii_whitespace(text: &str, mut index: usize) -> usize {
    while index < text.len() && text.as_bytes()[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

fn read_identifier(text: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = text.as_bytes();
    if !bytes.get(start).copied().is_some_and(is_identifier_start) {
        return None;
    }
    let mut end = start + 1;
    while end < bytes.len() && is_identifier_continue(bytes[end]) {
        end += 1;
    }
    text.get(start..end).map(|identifier| (identifier, end))
}

fn keyword_boundary(text: &str, start: usize, end: usize) -> bool {
    let bytes = text.as_bytes();
    let before_ok = start == 0 || !is_identifier_continue(bytes[start - 1]);
    let after_ok = end >= bytes.len() || !is_identifier_continue(bytes[end]);
    before_ok && after_ok
}

fn restart_segment_resets_score(segment: &str) -> bool {
    [
        "setscore(0",
        "setpoints(0",
        "setlevel(0",
        "score = 0",
        "score=0",
        "points = 0",
        "points=0",
    ]
    .iter()
    .any(|needle| segment.contains(needle))
        || segment_has_score_zero_assignment_reset(segment)
}

fn restart_segment_resets_entities(segment: &str) -> bool {
    [
        "setbullets([]",
        "setbullet([]",
        "setshots([]",
        "setprojectiles([]",
        "setenemies(",
        "setinvaders(",
        "setplayerstate(",
        "bullets = []",
        "bullets=[]",
        "enemies = []",
        "enemies=[]",
    ]
    .iter()
    .any(|needle| segment.contains(needle))
        || segment_has_generic_entity_fresh_reset(segment)
        || segment_has_entity_fresh_assignment_reset(segment)
}

fn segment_has_generic_entity_fresh_reset(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if !is_identifier_start(bytes[cursor]) {
            cursor += 1;
            continue;
        }
        let Some((name, name_end)) = read_identifier(segment, cursor) else {
            cursor += 1;
            continue;
        };
        let mut after_name = skip_ascii_whitespace(segment, name_end);
        if name.starts_with("set")
            && setter_name_has_entity_hint(name)
            && segment.as_bytes().get(after_name) == Some(&b'(')
        {
            after_name += 1;
            let argument_window_end = segment.len().min(after_name + 240);
            if let Some(argument_window) = segment.get(after_name..argument_window_end)
                && [
                    "[", "create", "initial", "init", "spawn", "make", "build", "default",
                ]
                .iter()
                .any(|needle| argument_window.contains(needle))
            {
                return true;
            }
        }
        cursor = name_end;
    }
    false
}

fn segment_has_entity_fresh_assignment_reset(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if !is_identifier_start(bytes[cursor]) {
            cursor += 1;
            continue;
        }
        let Some((name, name_end)) = read_identifier(segment, cursor) else {
            cursor += 1;
            continue;
        };
        if identifier_starts_property_access(segment, cursor) {
            cursor = name_end;
            continue;
        }
        if setter_name_has_entity_hint(name)
            && let Some(value_start) = assignment_value_start_after_identifier(segment, name_end)
            && assignment_value_starts_with_fresh_entity(segment, value_start)
        {
            return true;
        }
        cursor = name_end;
    }
    false
}

fn segment_has_score_zero_assignment_reset(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if !is_identifier_start(bytes[cursor]) {
            cursor += 1;
            continue;
        }
        let Some((name, name_end)) = read_identifier(segment, cursor) else {
            cursor += 1;
            continue;
        };
        if identifier_starts_property_access(segment, cursor) {
            cursor = name_end;
            continue;
        }
        if score_reset_name_has_progress_hint(name)
            && let Some(value_start) = assignment_value_start_after_identifier(segment, name_end)
            && assignment_value_starts_with_zero(segment, value_start)
        {
            return true;
        }
        cursor = name_end;
    }
    false
}

fn assignment_value_start_after_identifier(segment: &str, name_end: usize) -> Option<usize> {
    let bytes = segment.as_bytes();
    let mut cursor = skip_ascii_whitespace(segment, name_end);
    if bytes.get(cursor) == Some(&b'.') {
        cursor = skip_ascii_whitespace(segment, cursor + 1);
        let (property, property_end) = read_identifier(segment, cursor)?;
        if property != "current" {
            return None;
        }
        cursor = skip_ascii_whitespace(segment, property_end);
    }
    if !is_simple_assignment_operator(segment, cursor) {
        return None;
    }
    Some(skip_ascii_whitespace(segment, cursor + 1))
}

fn is_simple_assignment_operator(segment: &str, cursor: usize) -> bool {
    let bytes = segment.as_bytes();
    bytes.get(cursor) == Some(&b'=') && !matches!(bytes.get(cursor + 1), Some(&b'=') | Some(&b'>'))
}

fn identifier_starts_property_access(segment: &str, identifier_start: usize) -> bool {
    if identifier_start == 0 {
        return false;
    }
    segment.as_bytes()[..identifier_start]
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|previous| segment.as_bytes()[previous] == b'.')
}

fn assignment_value_starts_with_fresh_entity(segment: &str, value_start: usize) -> bool {
    let window_end = segment.len().min(value_start + 240);
    let Some(window) = segment.get(value_start..window_end) else {
        return false;
    };
    let window = window.trim_start();
    [
        "[", "{", "create", "initial", "init", "spawn", "make", "build", "default",
    ]
    .iter()
    .any(|needle| window.starts_with(needle))
        || window.starts_with("new ")
}

fn assignment_value_starts_with_zero(segment: &str, value_start: usize) -> bool {
    let bytes = segment.as_bytes();
    if bytes.get(value_start) != Some(&b'0') {
        return false;
    }
    !bytes
        .get(value_start + 1)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'.'))
}

fn score_reset_name_has_progress_hint(name: &str) -> bool {
    ["score", "points", "level", "progress"]
        .iter()
        .any(|needle| name.contains(needle))
}

fn setter_name_has_entity_hint(name: &str) -> bool {
    [
        "actor",
        "alien",
        "asteroid",
        "bullet",
        "cell",
        "enemy",
        "enemies",
        "entities",
        "entity",
        "invader",
        "laser",
        "missile",
        "mob",
        "obstacle",
        "player",
        "projectile",
        "ship",
        "shot",
        "target",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

fn restart_segment_references_recoverable_state(segment: &str) -> bool {
    [
        "gameover",
        "setgameover(",
        "setgamestate(",
        "setgamestate(\"playing\"",
        "setgamestate('playing'",
        "setgamestate(`playing`",
        "setplayerstate(",
    ]
    .iter()
    .any(|needle| segment.contains(needle))
}

fn restart_function_is_referenced_from_handler(lower: &str, name: &str) -> bool {
    handler_reference_segments(lower)
        .into_iter()
        .any(|segment| segment_contains_identifier(segment, name))
}

fn handler_reference_segments(lower: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    for needle in [
        "onclick",
        "onkeydown",
        "onkeyup",
        "onpointer",
        "onmousedown",
        "onmouseup",
        "ontouch",
        "onsubmit",
        "onchange",
        "pointerdown",
        "touchstart",
        "keydown",
        "keyup",
        "addeventlistener",
    ] {
        for (index, _) in lower.match_indices(needle) {
            let end = lower.len().min(index + 500);
            segments.push(&lower[index..end]);
        }
    }
    segments
}

fn segment_contains_identifier(segment: &str, name: &str) -> bool {
    let mut cursor = 0usize;
    while cursor < segment.len() {
        if !is_identifier_start(segment.as_bytes()[cursor]) {
            cursor += 1;
            continue;
        }
        let Some((identifier, identifier_end)) = read_identifier(segment, cursor) else {
            cursor += 1;
            continue;
        };
        if identifier == name {
            return true;
        }
        cursor = identifier_end;
    }
    false
}

fn restart_intent_function_name(name: &str) -> bool {
    ["restart", "reset", "init", "new"]
        .iter()
        .any(|needle| name.contains(needle))
        && ["game", "state", "level"]
            .iter()
            .any(|needle| name.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route_bound_space_invaders_component() -> &'static str {
        r#""use client";
import { useEffect, useState } from "react";
export default function SpaceInvaders(){
  const [score, setScore] = useState(0);
  const [gameState, setGameState] = useState("ready");
  const [enemies, setEnemies] = useState([{ x: 10, y: 20 }]);
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
  }, [enemies]);
  return <main><button onClick={() => setGameState("playing")}>Start</button><button onClick={() => { setGameState("ready"); setScore(0); setEnemies([{ x: 10, y: 20 }]); }}>Restart</button><canvas /><p>score {score} enemy collision {gameState}</p></main>;
}
"#
    }

    #[test]
    fn source_scanned_evidence_keys_report_source_channel() {
        for key in [
            "implementation_artifact",
            "interactive_ui_source_evidence",
            "non_static_screen_evidence",
            "visible_interactive_surface_evidence",
            "user_input_handler_evidence",
            "stateful_update_evidence",
            "challenge_or_adversary_evidence",
            "score_or_progression_evidence",
            "failure_or_collision_evidence",
            "restart_or_recoverable_state_evidence",
            "nextjs_route_evidence",
        ] {
            assert_eq!(
                evidence_satisfaction_channel(key),
                SatisfactionChannel::SourceScan,
                "{key}"
            );
        }
    }

    #[test]
    fn invader_interval_movement_satisfies_adversary_evidence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useEffect, useState } from "react";
export default function Page() {
  const [invaders, setInvaders] = useState([{ x: 10, y: 20 }]);
  useEffect(() => {
    const timer = setInterval(() => {
      setInvaders((current) => current.map((invader) => ({ ...invader, x: invader.x + 4 })));
    }, 80);
    return () => clearInterval(timer);
  }, []);
  return <main>{invaders.map((invader) => <span key={invader.x}>{invader.x}</span>)}</main>;
}
"#,
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &[],
            &["challenge_or_adversary_evidence".to_string()],
            &[],
            &[],
        );
        assert!(report.passed, "{report:?}");
        assert!(
            report.artifact_obligations[0]
                .evidence
                .contains(&"challenge_or_adversary_evidence".to_string())
        );
    }

    #[test]
    fn goal_hint_token_with_motion_satisfies_adversary_evidence_but_comment_only_does_not() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        let contract = crate::minimal_loop::completion::CompletionContract {
            required_paths: vec!["src/app/page.tsx".to_string()],
            verify_commands: Vec::new(),
            profile: None,
            goal: Some("シューティングでドラゴンを倒すゲーム".to_string()),
            required_capabilities: Vec::new(),
            deterministic_oracles: Vec::new(),
            required_evidence: vec!["challenge_or_adversary_evidence".to_string()],
            evidence_hint_tokens: Vec::new(),
            required_obligations: Vec::new(),
            deferred_verify_requirements: Vec::new(),
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap();
        assert!(
            contract
                .evidence_hint_tokens
                .contains(&"ドラゴン".to_string())
        );

        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useEffect, useState } from "react";
export default function Page() {
  const [ドラゴン, setドラゴン] = useState([{ x: 0, y: 20 }]);
  useEffect(() => {
    const timer = setInterval(() => {
      setドラゴン((current) => current.map((entity) => ({ ...entity, x: entity.x + 2 })));
    }, 100);
    return () => clearInterval(timer);
  }, []);
  return <main>{ドラゴン.map((entity) => <span key={entity.x}>{entity.x}</span>)}</main>;
}
"#,
        )
        .unwrap();
        let report = contract.runtime_acceptance_report(dir.path());
        assert!(report.passed, "{report:?}");
        assert!(
            report.artifact_obligations[0]
                .evidence
                .contains(&"challenge_or_adversary_evidence".to_string())
        );

        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
// ドラゴン
export default function Page() {
  return <main>ready</main>;
}
"#,
        )
        .unwrap();
        let report = contract.runtime_acceptance_report(dir.path());
        assert!(!report.passed, "{report:?}");
        assert!(
            report
                .missing_evidence
                .contains(&"challenge_or_adversary_evidence".to_string())
        );
    }

    #[test]
    fn non_source_evidence_keys_report_test_or_runtime_channels() {
        for key in [
            "test_artifact",
            "bound_verify_command",
            "non_zero_test_or_assertion_evidence",
            "requested_content_evidence",
        ] {
            assert_eq!(
                evidence_satisfaction_channel(key),
                SatisfactionChannel::TestArtifact,
                "{key}"
            );
        }
        for key in [
            "build_command_or_dependency_missing_boundary",
            "browser_readiness_failed:http_500",
            "interaction_evidence_missing",
            "unknown_future_runtime_evidence",
        ] {
            assert_eq!(
                evidence_satisfaction_channel(key),
                SatisfactionChannel::RuntimeArtifact,
                "{key}"
            );
        }
    }

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
    fn uat_0702_placeholder_space_invaders_source_is_weak_not_satisfied() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/SpaceInvaders.tsx"),
            r#""use client";
import { useEffect, useState } from "react";
type GameState = "playing" | "gameOver";
export default function SpaceInvaders() {
  const [score, setScore] = useState(0);
  const [gameState, setGameState] = useState<GameState>("playing");
  const enemies = [{ x: 20, y: 30 }, { x: 60, y: 30 }];
  const shootSound = { play() {} };
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.code === "Space") {
        // Basic shoot logic would go here.
        shootSound.play();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
  return <main><canvas /><p>Score: {score}</p><p>{gameState}</p><button>Restart</button>{enemies.map((enemy) => <span key={enemy.x}>{enemy.x}</span>)}</main>;
}
"#,
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/SpaceInvaders.tsx".to_string()],
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
        assert!(!report.passed, "{report:?}");
        for key in [
            "user_input_handler_evidence",
            "score_or_progression_evidence",
            "failure_or_collision_evidence",
            "restart_or_recoverable_state_evidence",
        ] {
            assert!(
                report.missing_evidence.contains(&key.to_string()),
                "{key}: {report:?}"
            );
            assert!(
                !report.artifact_obligations[0]
                    .evidence
                    .contains(&key.to_string()),
                "{key}: {report:?}"
            );
        }
        assert!(
            report
                .weak_evidence
                .iter()
                .any(|item| item.contains("weak_source_evidence:user_input_handler_evidence")),
            "{report:?}"
        );
        assert!(
            report
                .weak_evidence
                .iter()
                .any(|item| item.contains("weak_source_evidence:score_or_progression_evidence")),
            "{report:?}"
        );
    }

    #[test]
    fn wired_gameplay_source_satisfies_score_collision_restart_and_input() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useEffect, useState } from "react";
export default function Page() {
  const [score, setScore] = useState(0);
  const [gameOver, setGameOver] = useState(false);
  const [bullets, setBullets] = useState<{ x: number; y: number }[]>([]);
  const [enemies, setEnemies] = useState([{ x: 80, y: 20 }]);
  const fireBullet = () => setBullets((items) => [...items, { x: 50, y: 90 }]);
  const restart = () => {
    setGameOver(false);
    setScore(0);
    setBullets([]);
    setEnemies([{ x: 80, y: 20 }]);
  };
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.code === "Space") fireBullet();
    };
    const frame = requestAnimationFrame(() => {
      bullets.forEach((bullet) => {
        enemies.forEach((enemy) => {
          if (Math.abs(bullet.x - enemy.x) < 18 && Math.abs(bullet.y - enemy.y) < 18) {
            setGameOver(true);
            setScore((value) => value + 10);
          }
        });
      });
      setEnemies((items) => items.map((enemy) => ({ ...enemy, x: enemy.x + 1 })));
    });
    window.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [bullets, enemies]);
  return <main><canvas /><button onClick={restart}>Restart</button><p>score {score}</p><p>{gameOver ? "Game Over" : "Playing"}</p></main>;
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
        for key in [
            "user_input_handler_evidence",
            "score_or_progression_evidence",
            "failure_or_collision_evidence",
            "restart_or_recoverable_state_evidence",
        ] {
            assert!(evidence.contains(&key.to_string()), "{key}: {report:?}");
        }
        assert!(report.weak_evidence.is_empty(), "{report:?}");
    }

    #[test]
    fn named_init_game_restart_handler_satisfies_restart_evidence_regression() {
        let file = SourceFile::new(
            "src/app/page.tsx".to_string(),
            r#""use client";
import { useState } from "react";
type GameState = "START" | "PLAYING" | "GAMEOVER";
type Invader = { x: number; y: number };
function createInvaders(): Invader[] {
  return [{ x: 20, y: 30 }];
}
export default function Page() {
  const [score, setScore] = useState(0);
  const [bullets, setBullets] = useState<{ x: number; y: number }[]>([]);
  const [invaders, setInvaders] = useState<Invader[]>(createInvaders());
  const [gameState, setGameState] = useState<GameState>("START");
  const initGame = () => {
    setScore(0);
    setBullets([]);
    setInvaders(createInvaders());
    setGameState("PLAYING");
  };
  return (
    <main>
      <button onClick={initGame}>Restart</button>
      <p>{score} {bullets.length} {invaders.length} {gameState}</p>
    </main>
  );
}
"#
            .to_string(),
        );
        assert_eq!(
            source_file_restart_or_recoverable_state_signal(&file),
            SourceEvidenceSignal::Strong
        );
    }

    #[test]
    fn restart_handler_with_ref_held_entities_satisfies_restart_evidence_regression() {
        let file = SourceFile::new(
            "src/app/page.tsx".to_string(),
            r#""use client";
import { useRef, useState } from "react";
type GameState = "running" | "gameOver";
type Alien = { x: number; y: number };
function createAliens(level: number): Alien[] {
  return [{ x: level * 20, y: 24 }];
}
export default function Page() {
  const [score, setScore] = useState(0);
  const [gameState, setGameState] = useState<GameState>("gameOver");
  const aliensRef = useRef<Alien[]>(createAliens(1));
  const bulletsRef = useRef<{ x: number; y: number }[]>([]);
  const resetGame = () => {
    setScore(0);
    aliensRef.current = createAliens(1);
    bulletsRef.current = [];
    setGameState("running");
  };
  return (
    <main>
      <button onClick={resetGame}>Restart</button>
      <p>{score} {gameState} {aliensRef.current.length} {bulletsRef.current.length}</p>
    </main>
  );
}
"#
            .to_string(),
        );
        assert_eq!(
            source_file_restart_or_recoverable_state_signal(&file),
            SourceEvidenceSignal::Strong
        );
    }

    #[test]
    fn restart_reset_detection_accepts_ref_assignments_and_rejects_non_fresh_ref_rewrites() {
        assert!(restart_segment_resets_score("{ scoreref.current = 0; }"));
        assert!(restart_segment_resets_score("{ score.current=0; }"));

        assert!(restart_segment_resets_entities(
            "{ aliensref.current = createaliens(1); }"
        ));
        assert!(restart_segment_resets_entities(
            "{ bulletsref.current = []; }"
        ));
        assert!(restart_segment_resets_entities(
            "{ playerref.current = { x: 20, y: 30 }; }"
        ));
        assert!(restart_segment_resets_entities(
            "{ invaders.current=spawnwave(1); }"
        ));
        assert!(!restart_segment_resets_entities(
            "{ aliensref.current = aliensref.current.filter((alien) => alien.alive); }"
        ));

        assert!(restart_segment_resets_entities(
            "{ setinvaders(createinvaders()); }"
        ));
        assert!(restart_segment_resets_entities("{ bullets = []; }"));
    }

    #[test]
    fn label_only_restart_handler_remains_weak() {
        let source = r#""use client";
import { useState } from "react";
export default function Page() {
  const [gameState, setGameState] = useState("GAMEOVER");
  const initGame = () => {
    setGameState("GAMEOVER");
  };
  return <main><button onClick={initGame}>Restart</button><p>{gameState}</p></main>;
}
"#;
        let file = SourceFile::new("src/app/page.tsx".to_string(), source.to_string());
        assert!(matches!(
            source_file_restart_or_recoverable_state_signal(&file),
            SourceEvidenceSignal::Weak(reason)
                if reason.contains("resets score AND entities")
        ));

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(dir.path().join("src/app/page.tsx"), source).unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &[],
            &["restart_or_recoverable_state_evidence".to_string()],
            &[],
            &[],
        );
        assert!(!report.passed, "{report:?}");
        assert!(
            report
                .missing_evidence
                .contains(&"restart_or_recoverable_state_evidence".to_string())
        );
        assert!(
            report
                .weak_evidence
                .iter()
                .any(|item| item
                    .contains("weak_source_evidence:restart_or_recoverable_state_evidence")),
            "{report:?}"
        );
        assert_eq!(
            report
                .evidence_tiers
                .get("restart_or_recoverable_state_evidence")
                .map(String::as_str),
            Some("weak")
        );
        assert!(
            !report.artifact_obligations[0]
                .evidence
                .contains(&"restart_or_recoverable_state_evidence".to_string()),
            "{report:?}"
        );
    }

    #[test]
    fn comment_only_restart_mention_is_absent() {
        let file = SourceFile::new(
            "src/app/page.tsx".to_string(),
            r#""use client";
export default function Page() {
  // restart: setScore(0); setEnemies([]); setGameState("PLAYING");
  return <main />;
}
"#
            .to_string(),
        );
        assert_eq!(
            source_file_restart_or_recoverable_state_signal(&file),
            SourceEvidenceSignal::Absent
        );
    }

    #[test]
    fn inline_single_segment_restart_handler_still_satisfies_restart_evidence() {
        let file = SourceFile::new(
            "src/app/page.tsx".to_string(),
            r#""use client";
import { useState } from "react";
export default function Page() {
  const [score, setScore] = useState(0);
  const [gameState, setGameState] = useState("GAMEOVER");
  const [enemies, setEnemies] = useState([{ x: 10, y: 20 }]);
  return <main><button onClick={() => { setGameState("PLAYING"); setScore(0); setEnemies([{ x: 10, y: 20 }]); }}>Restart</button><p>{score} {gameState} {enemies.length}</p></main>;
}
"#
            .to_string(),
        );
        assert_eq!(
            source_file_restart_or_recoverable_state_signal(&file),
            SourceEvidenceSignal::Strong
        );
    }

    #[test]
    fn quoted_game_over_transition_inside_callback_is_strong_regression() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useEffect, useState } from "react";
export default function Page() {
  const [gameState, setGameState] = useState("playing");
  useEffect(() => {
    const collision = true;
    if (collision) setGameState('gameover');
  }, []);
  return <main>{gameState}</main>;
}
"#,
        )
        .unwrap();

        let file = SourceFile::new(
            "src/app/page.tsx".to_string(),
            std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap(),
        );
        assert_eq!(
            source_file_failure_or_collision_signal(&file),
            SourceEvidenceSignal::Strong
        );

        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &[],
            &["failure_or_collision_evidence".to_string()],
            &[],
            &[],
        );
        assert!(report.passed, "{report:?}");
        assert!(
            report
                .artifact_obligations
                .first()
                .is_some_and(|artifact| artifact
                    .evidence
                    .contains(&"failure_or_collision_evidence".to_string())),
            "{report:?}"
        );
        assert_eq!(
            report
                .evidence_tiers
                .get("failure_or_collision_evidence")
                .map(String::as_str),
            Some("strong")
        );
    }

    #[test]
    fn quoted_game_over_transition_inside_comment_is_absent_regression() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
export default function Page() {
  // if (collision) setGameState('gameover');
  return <main>ready</main>;
}
"#,
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &[],
            &["failure_or_collision_evidence".to_string()],
            &[],
            &[],
        );
        assert!(!report.passed, "{report:?}");
        assert!(
            report
                .missing_evidence
                .contains(&"failure_or_collision_evidence".to_string())
        );
        assert!(report.weak_evidence.is_empty(), "{report:?}");
        assert_eq!(
            report
                .evidence_tiers
                .get("failure_or_collision_evidence")
                .map(String::as_str),
            Some("absent")
        );
    }

    #[test]
    fn source_needle_families_survive_sourcefile_preprocessing() {
        struct Fixture {
            family: &'static str,
            path: &'static str,
            content: &'static str,
            required_evidence: &'static [&'static str],
            verify_commands: &'static [&'static str],
            evidence_hint_tokens: &'static [&'static str],
        }

        let fixtures = [
            Fixture {
                family: "implementation_artifact",
                path: "src/app/page.tsx",
                content: "export default function Page(){ return <main>ready</main>; }\n",
                required_evidence: &["implementation_artifact"],
                verify_commands: &[],
                evidence_hint_tokens: &[],
            },
            Fixture {
                family: "test_artifact_assertion_require_single_quote",
                path: "tests/page.test.js",
                content: "const assert = require('assert');\nassert.equal(1, 1);\n",
                required_evidence: &[
                    "test_artifact",
                    "non_zero_test_or_assertion_evidence",
                    "bound_verify_command",
                ],
                verify_commands: &["node tests/page.test.js"],
                evidence_hint_tokens: &[],
            },
            Fixture {
                family: "requested_content_readme",
                path: "README.md",
                content: "# Acceptance\nThe requested behavior is documented.\n",
                required_evidence: &["requested_content_evidence"],
                verify_commands: &[],
                evidence_hint_tokens: &[],
            },
            Fixture {
                family: "interactive_ui_use_state_click",
                path: "src/app/page.tsx",
                content: r#""use client";
import { useState } from "react";
export default function Page(){
  const [score, setScore] = useState(0);
  return <main onClick={() => setScore(score + 1)}>score {score}</main>;
}
"#,
                required_evidence: &["interactive_ui_source_evidence"],
                verify_commands: &[],
                evidence_hint_tokens: &[],
            },
            Fixture {
                family: "non_static_screen_score_use_state",
                path: "src/app/page.tsx",
                content: r#""use client";
import { useState } from "react";
export default function Page(){
  const [score] = useState(0);
  return <main>score {score}</main>;
}
"#,
                required_evidence: &["non_static_screen_evidence"],
                verify_commands: &[],
                evidence_hint_tokens: &[],
            },
            Fixture {
                family: "visible_surface_role_button_quoted",
                path: "src/app/page.tsx",
                content: "export default function Page(){ return <main role=\"button\">Play</main>; }\n",
                required_evidence: &["visible_interactive_surface_evidence"],
                verify_commands: &[],
                evidence_hint_tokens: &[],
            },
            Fixture {
                family: "user_input_handler_onclick_mutates",
                path: "src/app/page.tsx",
                content: r#""use client";
import { useState } from "react";
export default function Page(){
  const [score, setScore] = useState(0);
  return <button onClick={() => setScore(score + 1)}>Add</button>;
}
"#,
                required_evidence: &["user_input_handler_evidence"],
                verify_commands: &[],
                evidence_hint_tokens: &[],
            },
            Fixture {
                family: "stateful_update_usestate",
                path: "src/app/page.tsx",
                content: r#""use client";
import { useState } from "react";
export default function Page(){
  const [ready] = useState(true);
  return <main>{ready}</main>;
}
"#,
                required_evidence: &["stateful_update_evidence"],
                verify_commands: &[],
                evidence_hint_tokens: &[],
            },
            Fixture {
                family: "challenge_static_enemy",
                path: "src/app/page.tsx",
                content: "export default function Page(){ const enemy = { x: 1 }; return <main>{enemy.x}</main>; }\n",
                required_evidence: &["challenge_or_adversary_evidence"],
                verify_commands: &[],
                evidence_hint_tokens: &[],
            },
            Fixture {
                family: "challenge_goal_hint_motion",
                path: "src/app/page.tsx",
                content: "export default function Page(){ let dragonPosition = 0; dragonPosition += 1; return <main>{dragonPosition}</main>; }\n",
                required_evidence: &["challenge_or_adversary_evidence"],
                verify_commands: &[],
                evidence_hint_tokens: &["dragon"],
            },
            Fixture {
                family: "score_update_setscore",
                path: "src/app/page.tsx",
                content: r#""use client";
import { useState } from "react";
export default function Page(){
  const [score, setScore] = useState(0);
  setScore(1);
  return <main>score {score}</main>;
}
"#,
                required_evidence: &["score_or_progression_evidence"],
                verify_commands: &[],
                evidence_hint_tokens: &[],
            },
            Fixture {
                family: "failure_game_over_transition_single_quote",
                path: "src/app/page.tsx",
                content: r#""use client";
import { useState } from "react";
export default function Page(){
  const [gameState, setGameState] = useState("playing");
  const collision = true;
  if (collision) setGameState('gameover');
  return <main>{gameState}</main>;
}
"#,
                required_evidence: &["failure_or_collision_evidence"],
                verify_commands: &[],
                evidence_hint_tokens: &[],
            },
            Fixture {
                family: "failure_collision_conditional",
                path: "src/app/page.tsx",
                content: "export default function Page(){ const collisionDistance = 10; const player = { x: 0, y: 0 }; const enemy = { x: 1, y: 1 }; if (Math.abs(player.x - enemy.x) < collisionDistance) return <main>hit</main>; return <main />; }\n",
                required_evidence: &["failure_or_collision_evidence"],
                verify_commands: &[],
                evidence_hint_tokens: &[],
            },
            Fixture {
                family: "restart_recoverable_state_playing_single_quote",
                path: "src/app/page.tsx",
                content: r#""use client";
import { useState } from "react";
export default function Page(){
  const [score, setScore] = useState(3);
  const [gameOver] = useState(true);
  const [enemies, setEnemies] = useState([{ x: 1 }]);
  return <button onClick={() => { setGameState('playing'); setScore(0); setEnemies([{ x: 1 }]); }}>Restart {score} {String(gameOver)} {enemies.length}</button>;
}
"#,
                required_evidence: &["restart_or_recoverable_state_evidence"],
                verify_commands: &[],
                evidence_hint_tokens: &[],
            },
            Fixture {
                family: "nextjs_route_page",
                path: "src/app/page.tsx",
                content: "export default function Page(){ return <main>route</main>; }\n",
                required_evidence: &["nextjs_route_evidence"],
                verify_commands: &[],
                evidence_hint_tokens: &[],
            },
        ];

        for fixture in fixtures {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(fixture.path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, fixture.content).unwrap();
            let report = verify_runtime_acceptance_with_hints(
                dir.path(),
                &[fixture.path.to_string()],
                &fixture
                    .verify_commands
                    .iter()
                    .map(|command| command.to_string())
                    .collect::<Vec<_>>(),
                &[],
                &fixture
                    .required_evidence
                    .iter()
                    .map(|evidence| evidence.to_string())
                    .collect::<Vec<_>>(),
                &[],
                &[],
                &fixture
                    .evidence_hint_tokens
                    .iter()
                    .map(|token| token.to_string())
                    .collect::<Vec<_>>(),
            );
            assert!(report.passed, "{}: {report:?}", fixture.family);
            for evidence in fixture.required_evidence {
                assert_eq!(
                    report.evidence_tiers.get(*evidence).map(String::as_str),
                    Some("strong"),
                    "{}: {evidence}: {report:?}",
                    fixture.family
                );
            }
        }
    }

    #[test]
    fn gameplay_keywords_inside_comments_or_strings_do_not_satisfy_source_scan() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
export default function Page() {
  // score += 10; setScore(10); collision gameOver restart
  const label = "score setScore collision gameOver restart";
  return <main>{label}</main>;
}
"#,
        )
        .unwrap();
        let comment_only = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &[],
            &["score_or_progression_evidence".to_string()],
            &[],
            &[],
        );
        assert!(!comment_only.passed, "{comment_only:?}");
        assert!(
            comment_only
                .missing_evidence
                .contains(&"score_or_progression_evidence".to_string())
        );
        assert!(comment_only.weak_evidence.is_empty(), "{comment_only:?}");

        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
export default function Page() {
  let score = 0;
  score += 10;
  return <main>{score}</main>;
}
"#,
        )
        .unwrap();
        let code_signal = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &[],
            &["score_or_progression_evidence".to_string()],
            &[],
            &[],
        );
        assert!(code_signal.passed, "{code_signal:?}");
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
  const [enemies, setEnemies] = useState([{ x: 10, y: 20 }]);
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
  return <main><button onClick={() => setGameState("playing")}>Start</button><button onClick={() => { setGameState("ready"); setScore(0); setEnemies([{ x: 10, y: 20 }]); }}>Restart</button><canvas /><p>score {score} enemy collision {gameState}</p></main>;
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
    fn route_unbound_game_component_does_not_satisfy_final_capability_evidence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/components")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
export default function Page(){
  return <main><button>Start</button></main>;
}
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/components/SpaceInvaders.tsx"),
            route_bound_space_invaders_component(),
        )
        .unwrap();

        let report = verify_runtime_acceptance(
            dir.path(),
            &[
                "src/app/page.tsx".to_string(),
                "src/components/SpaceInvaders.tsx".to_string(),
            ],
            &[],
            &[
                "player_control".to_string(),
                "progression_or_score".to_string(),
            ],
            &[],
            &["implementation".to_string()],
            &[],
        );

        assert!(!report.passed, "{report:?}");
        assert!(
            report
                .missing_evidence
                .contains(&"user_input_handler_evidence".to_string()),
            "{report:?}"
        );
        assert!(
            report
                .weak_evidence
                .contains(&"route_unbound:src/components/SpaceInvaders.tsx".to_string()),
            "{report:?}"
        );
        assert!(
            report.diagnostics.contains(
                &"route_unbound_capability_artifact:src/components/SpaceInvaders.tsx".to_string()
            ),
            "{report:?}"
        );
        let binding = report
            .capability_evidence_bindings
            .iter()
            .find(|binding| binding.capability == "player_control")
            .expect("player control binding");
        assert!(
            !binding
                .artifact_paths
                .contains(&"src/components/SpaceInvaders.tsx".to_string()),
            "{binding:?}"
        );
    }

    #[test]
    fn route_imported_game_component_satisfies_final_capability_evidence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/components")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import SpaceInvaders from "../components/SpaceInvaders";
export default function Page(){ return <SpaceInvaders />; }
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/components/SpaceInvaders.tsx"),
            route_bound_space_invaders_component(),
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
            &["implementation".to_string()],
            &[],
        );

        assert!(report.passed, "{report:?}");
        assert!(report.diagnostics.is_empty(), "{report:?}");
    }

    #[test]
    fn alias_imported_game_component_satisfies_final_capability_evidence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/components")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import SpaceInvaders from "@/components/SpaceInvaders";
export default function Page(){ return <SpaceInvaders />; }
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/components/SpaceInvaders.tsx"),
            route_bound_space_invaders_component(),
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
            &["implementation".to_string()],
            &[],
        );

        assert!(report.passed, "{report:?}");
        assert!(report.diagnostics.is_empty(), "{report:?}");
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
        assert_eq!(
            report.interaction_evidence_status,
            "unavailable:playwright_not_installed"
        );
        assert!(!report.inconclusive_reasons.iter().any(|reason| {
            reason.contains("browser_interaction_unavailable:interaction_evidence_missing")
        }));
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
            r#"{"ok":true,"interaction_performed":true,"start_transition":true,"input_state_change":true,"input_event_observed":true,"state_changed":true}"#,
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
    fn browser_interaction_accepts_startless_input_state_change() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useEffect, useState } from "react";
export default function Page(){
  const [draft,setDraft] = useState("");
  const [items,setItems] = useState<string[]>([]);
  useEffect(() => {
    localStorage.setItem("todos", JSON.stringify(items));
  }, [items]);
  return <main>
    <input aria-label="Todo" value={draft} onChange={(event) => setDraft(event.target.value)} />
    <button onClick={() => setItems([...items, draft])}>Add</button>
    <p>{draft}</p>
  </main>;
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
            r#"{"ok":true,"interaction_performed":true,"surface_visible":true,"start_control_found":false,"input_state_change":true,"input_event_observed":true,"state_changed":true,"steps":["surface_visible","control_input_dispatched","input_state_change"]}"#,
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &["browser_interaction".to_string(), "persistence".to_string()],
            &[],
            &[],
            &[],
        );
        assert!(report.passed, "{report:?}");
        assert_eq!(report.interaction_evidence_status, "passed");
        assert_eq!(
            report.evidence_tiers.get("persistence_evidence"),
            Some(&"strong".to_string())
        );
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
