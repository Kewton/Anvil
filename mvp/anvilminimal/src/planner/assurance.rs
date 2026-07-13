use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReleaseGateSummary {
    pub(super) status: String,
    pub(super) reasons: Vec<String>,
    pub(super) browser_readiness_status: String,
    pub(super) browser_readiness_evidence_path: String,
    pub(super) interaction_evidence_status: String,
    pub(super) interaction_evidence_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AcceptanceGateTelemetry {
    pub(super) browser_readiness_applicable: bool,
    pub(super) browser_readiness_execution_status: String,
    pub(super) interaction_evidence_applicable: bool,
    pub(super) interaction_evidence_execution_status: String,
}

pub(super) fn production_build_failed_release_gate() -> ReleaseGateSummary {
    ReleaseGateSummary {
        status: "not_applicable".to_string(),
        reasons: vec!["production_build_failed_before_browser_probe".to_string()],
        browser_readiness_status: "not_applicable".to_string(),
        browser_readiness_evidence_path: String::new(),
        interaction_evidence_status: "not_applicable".to_string(),
        interaction_evidence_path: String::new(),
    }
}

pub(super) fn final_acceptance_release_gate(
    config: &Config,
    profile: &str,
    goal: &str,
    required_capabilities: &[String],
    acceptance: Option<&crate::minimal_loop::evidence::RuntimeAcceptanceReport>,
    check_browser_on_runtime_failure: bool,
) -> ReleaseGateSummary {
    let effective_profile = canonical_profile_name(profile);
    let is_next = effective_profile == "nextjs";
    let acceptance_required_evidence = acceptance
        .map(|report| report.evidence_tiers.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let interaction_options =
        browser_interaction_probe_options(required_capabilities, &acceptance_required_evidence);
    let requested_port = effective_requested_port(&effective_profile, goal, None);
    let requires_browser = is_next
        && (required_capabilities.iter().any(|capability| {
            matches!(
                capability.as_str(),
                "stateful_interaction"
                    | "player_control"
                    | "user_input_or_action"
                    | "visible_state_change"
                    | "persistence"
                    | "adversary_or_challenge"
                    | "progression_or_score"
                    | "failure_or_collision_rule"
            )
        }) || signals::contains_browser_probe_token(goal));
    let Some(report) = acceptance else {
        return ReleaseGateSummary {
            status: "not_applicable".to_string(),
            reasons: Vec::new(),
            browser_readiness_status: "not_applicable".to_string(),
            browser_readiness_evidence_path: String::new(),
            interaction_evidence_status: "not_applicable".to_string(),
            interaction_evidence_path: String::new(),
        };
    };
    if !report.passed {
        if requires_browser
            && check_browser_on_runtime_failure
            && runtime_acceptance_has_buildable_nextjs_boundary(report)
        {
            let mut gate = browser_release_gate_with_options(
                config,
                requires_canvas_surface(goal, required_capabilities),
                interaction_options,
                requested_port.as_ref().map(|requested| requested.port),
            );
            let mut reasons = vec![report.primary_reason.clone()];
            reasons.extend(std::mem::take(&mut gate.reasons));
            gate.status = "failed".to_string();
            gate.reasons = dedup_strings(reasons);
            return gate;
        }
        return ReleaseGateSummary {
            status: "failed".to_string(),
            reasons: vec![report.primary_reason.clone()],
            browser_readiness_status: "not_checked".to_string(),
            browser_readiness_evidence_path: String::new(),
            interaction_evidence_status: "not_checked".to_string(),
            interaction_evidence_path: String::new(),
        };
    }
    if !report.unverified_evidence.is_empty() {
        let mut gate = if requires_browser {
            browser_release_gate_with_options(
                config,
                requires_canvas_surface(goal, required_capabilities),
                interaction_options,
                requested_port.as_ref().map(|requested| requested.port),
            )
        } else {
            ReleaseGateSummary {
                status: "partial".to_string(),
                reasons: Vec::new(),
                browser_readiness_status: "not_applicable".to_string(),
                browser_readiness_evidence_path: String::new(),
                interaction_evidence_status: "not_applicable".to_string(),
                interaction_evidence_path: String::new(),
            }
        };
        let mut reasons = runtime_acceptance_unverified_release_reasons(
            report,
            interaction_probe_performed_for_run(config),
        );
        reasons.extend(std::mem::take(&mut gate.reasons));
        gate.status = "partial".to_string();
        gate.reasons = dedup_strings(reasons);
        return gate;
    }
    if requires_browser {
        return browser_release_gate_with_options(
            config,
            requires_canvas_surface(goal, required_capabilities),
            interaction_options,
            requested_port.as_ref().map(|requested| requested.port),
        );
    }
    ReleaseGateSummary {
        status: "pass".to_string(),
        reasons: Vec::new(),
        browser_readiness_status: "not_applicable".to_string(),
        browser_readiness_evidence_path: String::new(),
        interaction_evidence_status: "not_applicable".to_string(),
        interaction_evidence_path: String::new(),
    }
}

pub(super) fn acceptance_gate_telemetry(
    profile: &str,
    signal_text: &str,
    required_capabilities: &[String],
    required_evidence: &[String],
    release_gate: &ReleaseGateSummary,
) -> AcceptanceGateTelemetry {
    let browser_applicable =
        ultra_browser_probe_required(profile, signal_text, required_capabilities);
    let interaction_applicable =
        browser_applicable && interaction_gate_required(required_capabilities, required_evidence);
    AcceptanceGateTelemetry {
        browser_readiness_applicable: browser_applicable,
        browser_readiness_execution_status: gate_execution_status(
            &release_gate.browser_readiness_status,
        ),
        interaction_evidence_applicable: interaction_applicable,
        interaction_evidence_execution_status: gate_execution_status(
            &release_gate.interaction_evidence_status,
        ),
    }
}

pub(super) fn interaction_gate_required(
    required_capabilities: &[String],
    required_evidence: &[String],
) -> bool {
    required_capabilities
        .iter()
        .chain(required_evidence.iter())
        .any(|requirement| {
            matches!(
                requirement.as_str(),
                "stateful_interaction"
                    | "player_control"
                    | "user_input_or_action"
                    | "visible_state_change"
                    | "persistence"
                    | "adversary_or_challenge"
                    | "progression_or_score"
                    | "failure_or_collision_rule"
                    | "browser_interaction"
                    | "playable_ui"
                    | "interactive_ui_source_evidence"
                    | "visible_interactive_surface_evidence"
                    | "user_input_handler_evidence"
                    | "stateful_update_evidence"
                    | "non_static_screen_evidence"
                    | "persistence_evidence"
                    | "challenge_or_adversary_evidence"
                    | "score_or_progression_evidence"
                    | "failure_or_collision_evidence"
            )
        })
}

pub(super) fn gate_execution_status(status: &str) -> String {
    if gate_status_disconnected(status) {
        "disconnected".to_string()
    } else if status == "passed" {
        "performed".to_string()
    } else if status.starts_with("failed") {
        "performed_failed".to_string()
    } else if status.starts_with("unavailable") {
        "unavailable".to_string()
    } else {
        status.to_string()
    }
}

pub(super) fn gate_status_disconnected(status: &str) -> bool {
    let status = status.trim();
    status.is_empty()
        || matches!(status, "not_applicable" | "not_checked" | "skipped")
        || status.starts_with("skipped:")
}

pub(super) fn acceptance_gates_disconnected_reason(
    telemetry: &AcceptanceGateTelemetry,
    release_gate: &ReleaseGateSummary,
) -> Option<String> {
    let mut disconnected = Vec::new();
    if telemetry.browser_readiness_applicable
        && gate_status_disconnected(&release_gate.browser_readiness_status)
    {
        disconnected.push(format!(
            "browser_readiness_status={}",
            release_gate.browser_readiness_status
        ));
    }
    if telemetry.interaction_evidence_applicable
        && gate_status_disconnected(&release_gate.interaction_evidence_status)
    {
        disconnected.push(format!(
            "interaction_evidence_status={}",
            release_gate.interaction_evidence_status
        ));
    }
    (!disconnected.is_empty())
        .then(|| format!("acceptance_gates_disconnected:{}", disconnected.join(",")))
}

pub(super) fn mark_release_gate_profile_behavior_failed(
    release_gate: &mut ReleaseGateSummary,
    profile_behavior_probe: &ProfileBehaviorProbeReport,
) {
    let mut reasons = release_gate.reasons.clone();
    if profile_behavior_probe.reasons.is_empty() {
        reasons.push("profile_behavior_probe_failed".to_string());
    } else {
        reasons.extend(
            profile_behavior_probe
                .reasons
                .iter()
                .map(|reason| format!("profile_behavior_probe_failed:{reason}")),
        );
    }
    if let Some(path) = &profile_behavior_probe.evidence_path {
        reasons.push(format!("profile_behavior_probe_evidence:{path}"));
    }
    release_gate.status = "failed".to_string();
    release_gate.reasons = dedup_strings(reasons);
}

pub(super) fn runtime_acceptance_unverified_release_reasons(
    report: &crate::minimal_loop::evidence::RuntimeAcceptanceReport,
    probe_performed_for_run: bool,
) -> Vec<String> {
    let mut reasons = Vec::new();
    let mut saw_probe_unavailable = false;
    for evidence in &report.unverified_evidence {
        if let Some(reason) = evidence
            .split_once(":unverified:")
            .map(|(_, reason)| reason.trim())
            .filter(|reason| !reason.is_empty())
        {
            if reason == "probe_unavailable" {
                if probe_performed_for_run {
                    continue;
                }
                saw_probe_unavailable = true;
            } else {
                reasons.push(format!("interaction_unverified:{reason}"));
            }
        }
        if probe_performed_for_run && evidence.contains(":unverified:probe_unavailable") {
            continue;
        }
        reasons.push(format!("unverified_probe_required:{evidence}"));
    }
    if saw_probe_unavailable {
        reasons.insert(0, "interaction_unverified:probe_unavailable".to_string());
        reasons.push(
            crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION.to_string(),
        );
    }
    reasons
}

pub(super) fn interaction_probe_performed_for_run(config: &Config) -> bool {
    release_evidence_candidate_paths(
        config,
        &[
            "browser-interaction.json",
            "interaction-evidence.json",
            "interaction.json",
        ],
    )
    .into_iter()
    .filter_map(|path| std::fs::read_to_string(path).ok())
    .filter_map(|text| serde_json::from_str::<Value>(&text).ok())
    .any(|value| {
        let details = value
            .get("browser_details")
            .or_else(|| value.get("details"))
            .filter(|value| value.is_object());
        bool_field_deep(&value, details, &["interaction_performed"]) == Some(true)
    })
}

pub(super) fn runtime_acceptance_has_buildable_nextjs_boundary(
    report: &crate::minimal_loop::evidence::RuntimeAcceptanceReport,
) -> bool {
    !report.missing_evidence.iter().any(|item| {
        matches!(
            item.as_str(),
            "implementation_artifact"
                | "nextjs_route_evidence"
                | "build_command_or_dependency_missing_boundary"
        )
    }) && !report
        .missing_obligations
        .iter()
        .any(|item| item == "implementation")
}

pub(super) fn requires_canvas_surface(signal_text: &str, required_capabilities: &[String]) -> bool {
    signals::contains_canvas_token(signal_text)
        && required_capabilities.iter().any(|capability| {
            matches!(
                capability.as_str(),
                "browser_interaction"
                    | "playable_ui"
                    | "stateful_interaction"
                    | "player_control"
                    | "user_input_or_action"
                    | "visible_state_change"
                    | "adversary_or_challenge"
                    | "progression_or_score"
                    | "failure_or_collision_rule"
            )
        })
}

pub(super) fn append_release_gate_observation_failures(
    report: &mut VerificationReport,
    release_gate: &ReleaseGateSummary,
) {
    if release_gate.browser_readiness_status != "not_checked"
        && release_gate.browser_readiness_status != "not_applicable"
    {
        report.push_profile_failure(format!(
            "browser readiness status: {}",
            release_gate.browser_readiness_status
        ));
    }
    if !release_gate.browser_readiness_evidence_path.is_empty() {
        report.push_profile_failure(format!(
            "browser readiness evidence: {}",
            release_gate.browser_readiness_evidence_path
        ));
        if release_gate
            .browser_readiness_status
            .contains("build_verifier_failed")
        {
            report.push_compile_errors(
                "browser readiness build verifier",
                compile_errors_from_release_evidence_path(
                    &release_gate.browser_readiness_evidence_path,
                ),
            );
        }
    }
    if release_gate.interaction_evidence_status != "not_checked"
        && release_gate.interaction_evidence_status != "not_applicable"
    {
        report.push_profile_failure(format!(
            "interaction evidence status: {}",
            release_gate.interaction_evidence_status
        ));
    }
    if !release_gate.interaction_evidence_path.is_empty() {
        report.push_profile_failure(format!(
            "interaction evidence path: {}",
            release_gate.interaction_evidence_path
        ));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReleaseEvidenceStatus {
    Passed,
    Failed(String),
    Unavailable(String),
}

impl ReleaseEvidenceStatus {
    fn as_status(&self) -> String {
        match self {
            Self::Passed => "passed".to_string(),
            Self::Failed(reason) => format!("failed:{reason}"),
            Self::Unavailable(reason) => format!("unavailable:{reason}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReleaseEvidence {
    status: ReleaseEvidenceStatus,
    path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReleaseEvidenceKind {
    BrowserReadiness,
    Interaction,
}

#[cfg(test)]
pub(super) fn browser_release_gate(config: &Config) -> ReleaseGateSummary {
    browser_release_gate_with_expectations(config, false)
}

#[cfg(test)]
pub(super) fn browser_release_gate_with_expectations(
    config: &Config,
    canvas_surface_expected: bool,
) -> ReleaseGateSummary {
    browser_release_gate_with_options(
        config,
        canvas_surface_expected,
        BrowserInteractionProbeOptions::default(),
        None,
    )
}

pub(super) fn browser_release_gate_with_options(
    config: &Config,
    canvas_surface_expected: bool,
    interaction_options: BrowserInteractionProbeOptions,
    requested_port: Option<u16>,
) -> ReleaseGateSummary {
    let mut browser = read_release_evidence(
        config,
        &[
            "browser-readiness.json",
            "browser.json",
            "browser-readiness-evidence.json",
        ],
        "browser_readiness_evidence_missing",
        ReleaseEvidenceKind::BrowserReadiness,
    );
    if matches!(
        &browser.status,
        ReleaseEvidenceStatus::Unavailable(reason)
            if reason == "browser_readiness_evidence_missing"
    ) {
        browser = nextjs_dev_route_release_evidence(config, interaction_options, requested_port);
    }
    let interaction = read_release_evidence(
        config,
        &[
            "browser-interaction.json",
            "interaction-evidence.json",
            "interaction.json",
        ],
        "interaction_evidence_missing",
        ReleaseEvidenceKind::Interaction,
    );
    let browser_status = browser.status.as_status();
    let mut interaction_status = interaction.status.as_status();
    let canvas_surface_missing =
        release_gate_canvas_surface_missing(canvas_surface_expected, &browser, &interaction);
    if let ReleaseEvidenceStatus::Failed(reason) = &browser.status {
        if matches!(interaction.status, ReleaseEvidenceStatus::Unavailable(_)) {
            interaction_status = format!("not_exercised:{reason}");
        }
        return ReleaseGateSummary {
            status: "failed".to_string(),
            reasons: vec![format!("browser_readiness_failed:{reason}")],
            browser_readiness_status: browser_status,
            browser_readiness_evidence_path: browser.path,
            interaction_evidence_status: interaction_status,
            interaction_evidence_path: interaction.path,
        };
    }
    if let ReleaseEvidenceStatus::Failed(reason) = &interaction.status {
        if interaction_probe_infrastructure_failure_reason(reason) {
            let mut reasons = vec![
                reason.clone(),
                format!("app interaction untested (probe infrastructure failure: {reason})"),
            ];
            if let Some(remediation) = interaction_probe_failure_remediation(&interaction.path) {
                reasons.push(remediation);
            } else if reason == "probe_dependency_missing:browser_binaries_missing" {
                reasons.push(
                    crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION
                        .to_string(),
                );
            }
            return ReleaseGateSummary {
                status: "failed".to_string(),
                reasons: dedup_strings(reasons),
                browser_readiness_status: browser_status,
                browser_readiness_evidence_path: browser.path,
                interaction_evidence_status: interaction_status,
                interaction_evidence_path: interaction.path,
            };
        }
        return ReleaseGateSummary {
            status: "failed".to_string(),
            reasons: vec![format!("browser_interaction_failed:{reason}")],
            browser_readiness_status: browser_status,
            browser_readiness_evidence_path: browser.path,
            interaction_evidence_status: interaction_status,
            interaction_evidence_path: interaction.path,
        };
    }
    if let ReleaseEvidenceStatus::Unavailable(reason) = &browser.status {
        return ReleaseGateSummary {
            status: "partial".to_string(),
            reasons: vec![format!(
                "browser_readiness_or_interaction_evidence_required:{reason}"
            )],
            browser_readiness_status: browser_status,
            browser_readiness_evidence_path: browser.path,
            interaction_evidence_status: interaction_status,
            interaction_evidence_path: interaction.path,
        };
    }
    if let ReleaseEvidenceStatus::Unavailable(reason) = &interaction.status {
        let mut reasons = Vec::new();
        if canvas_surface_missing {
            reasons.push(
                "browser_readiness_or_interaction_evidence_required:rendered_without_expected_surface"
                    .to_string(),
            );
        }
        if interaction_probe_unavailable_reason_value(reason) {
            if interaction_probe_performed_for_run(config) {
                reasons.push(
                    "browser_interaction_evidence_required:interaction_detail_missing".to_string(),
                );
                return ReleaseGateSummary {
                    status: "partial".to_string(),
                    reasons: dedup_strings(reasons),
                    browser_readiness_status: browser_status,
                    browser_readiness_evidence_path: browser.path,
                    interaction_evidence_status: interaction_status,
                    interaction_evidence_path: interaction.path,
                };
            }
            reasons.extend([
                "interaction_unverified:probe_unavailable".to_string(),
                crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION
                    .to_string(),
            ]);
            return ReleaseGateSummary {
                status: "partial".to_string(),
                reasons: dedup_strings(reasons),
                browser_readiness_status: browser_status,
                browser_readiness_evidence_path: browser.path,
                interaction_evidence_status: interaction_status,
                interaction_evidence_path: interaction.path,
            };
        }
        reasons.push(format!("browser_interaction_evidence_required:{reason}"));
        return ReleaseGateSummary {
            status: "partial".to_string(),
            reasons: dedup_strings(reasons),
            browser_readiness_status: browser_status,
            browser_readiness_evidence_path: browser.path,
            interaction_evidence_status: interaction_status,
            interaction_evidence_path: interaction.path,
        };
    }
    ReleaseGateSummary {
        status: "pass".to_string(),
        reasons: Vec::new(),
        browser_readiness_status: browser_status,
        browser_readiness_evidence_path: browser.path,
        interaction_evidence_status: interaction_status,
        interaction_evidence_path: interaction.path,
    }
}

pub(super) fn nextjs_dev_route_release_evidence(
    config: &Config,
    interaction_options: BrowserInteractionProbeOptions,
    requested_port: Option<u16>,
) -> ReleaseEvidence {
    let path = nextjs_dev_route_evidence_path(config);
    let value = run_nextjs_dev_route_probe_with_interaction_options(
        config,
        &path,
        interaction_options,
        requested_port,
    );
    let status = classify_release_evidence_json(ReleaseEvidenceKind::BrowserReadiness, &value);
    write_release_evidence_json(&path, &value);
    ReleaseEvidence {
        status,
        path: path.display().to_string(),
    }
}

pub(super) fn write_release_evidence_json(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(value) {
        let _ = std::fs::write(path, format!("{text}\n"));
    }
}

pub(super) fn interaction_probe_infrastructure_failure_reason(reason: &str) -> bool {
    reason.starts_with("probe_dependency_missing")
        || reason.starts_with("probe_infrastructure_failed")
}

pub(super) fn release_gate_has_interaction_probe_infrastructure_failure(
    release_gate: &ReleaseGateSummary,
) -> bool {
    release_gate
        .interaction_evidence_status
        .strip_prefix("failed:")
        .is_some_and(interaction_probe_infrastructure_failure_reason)
        || release_gate
            .reasons
            .iter()
            .any(|reason| interaction_probe_infrastructure_failure_reason(reason))
}

pub(super) fn interaction_probe_failure_remediation(path: &str) -> Option<String> {
    let value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())?;
    let details = value
        .get("browser_details")
        .or_else(|| value.get("details"))
        .filter(|value| value.is_object());
    text_field_deep(&value, details, &["remediation"]).filter(|remediation| !remediation.is_empty())
}

pub(super) fn nextjs_dev_route_evidence_path(config: &Config) -> PathBuf {
    if let Some(events_path) = &config.eval_events_path
        && let Some(run_dir) = events_path.parent()
    {
        return run_dir.join("browser-readiness.json");
    }
    config
        .workspace_root
        .join(".anvil")
        .join("browser-readiness.json")
}

pub(super) fn release_evidence_canvas_marker_is_false(path: &str) -> bool {
    let value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    value.as_ref().is_some_and(|value| {
        let details = value
            .get("browser_details")
            .or_else(|| value.get("details"))
            .filter(|value| value.is_object());
        bool_field_deep(value, details, &["ssr_has_canvas", "has_canvas"]) == Some(false)
    })
}

pub(super) fn release_gate_canvas_surface_missing(
    expected: bool,
    browser: &ReleaseEvidence,
    interaction: &ReleaseEvidence,
) -> bool {
    if !expected {
        return false;
    }
    if release_interaction_surface_authoritative(interaction) {
        return release_interaction_canvas_marker(&interaction.path) == Some(false);
    }
    matches!(browser.status, ReleaseEvidenceStatus::Passed)
        && release_evidence_canvas_marker_is_false(&browser.path)
}

pub(super) fn release_interaction_surface_authoritative(interaction: &ReleaseEvidence) -> bool {
    if interaction.path.is_empty() {
        return false;
    }
    match &interaction.status {
        ReleaseEvidenceStatus::Passed => true,
        ReleaseEvidenceStatus::Failed(reason) => {
            !interaction_probe_infrastructure_failure_reason(reason)
        }
        ReleaseEvidenceStatus::Unavailable(_) => false,
    }
}

pub(super) fn release_interaction_canvas_marker(path: &str) -> Option<bool> {
    let value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())?;
    let details = value
        .get("browser_details")
        .or_else(|| value.get("details"))
        .filter(|value| value.is_object());
    if let Some(has_canvas) = bool_field_deep(
        &value,
        details,
        &[
            "post_js_has_canvas",
            "has_canvas",
            "canvas_found",
            "canvas_available",
        ],
    ) {
        return Some(has_canvas);
    }
    numeric_field_deep(&value, details, &["post_js_canvas_count", "canvas_count"])
        .map(|count| count > 0)
}

pub(super) fn read_release_evidence(
    config: &Config,
    names: &[&str],
    missing_reason: &'static str,
    kind: ReleaseEvidenceKind,
) -> ReleaseEvidence {
    for path in release_evidence_candidate_paths(config, names) {
        if !path.is_file() {
            continue;
        }
        let display = path.display().to_string();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return ReleaseEvidence {
                status: ReleaseEvidenceStatus::Failed("evidence_unreadable".to_string()),
                path: display,
            };
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return ReleaseEvidence {
                status: ReleaseEvidenceStatus::Failed("evidence_invalid_json".to_string()),
                path: display,
            };
        };
        return ReleaseEvidence {
            status: classify_release_evidence_json(kind, &json),
            path: display,
        };
    }
    if kind == ReleaseEvidenceKind::Interaction
        && let Some(reason) = interaction_probe_unavailable_reason(&config.workspace_root)
    {
        return ReleaseEvidence {
            status: ReleaseEvidenceStatus::Unavailable(reason),
            path: String::new(),
        };
    }
    ReleaseEvidence {
        status: ReleaseEvidenceStatus::Unavailable(missing_reason.to_string()),
        path: String::new(),
    }
}

pub(super) fn interaction_probe_unavailable_reason(root: &Path) -> Option<String> {
    interaction_probe::playwright_availability(root)
        .unavailable_reason()
        .map(str::to_string)
}

pub(super) fn interaction_probe_unavailable_reason_value(reason: &str) -> bool {
    matches!(reason, "playwright_not_installed" | "probe_unavailable")
}

pub(super) fn release_evidence_candidate_paths(config: &Config, names: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(events_path) = &config.eval_events_path
        && let Some(run_dir) = events_path.parent()
    {
        for name in names {
            out.push(run_dir.join(name));
        }
    }
    for name in names {
        out.push(
            config
                .workspace_root
                .join(".anvil")
                .join("evidence")
                .join(name),
        );
        out.push(config.workspace_root.join(".anvil").join(name));
        out.push(config.workspace_root.join(name));
    }
    out
}

pub(super) fn release_evidence_extra_dirs(config: &Config) -> Vec<PathBuf> {
    config
        .eval_events_path
        .as_ref()
        .and_then(|events_path| events_path.parent())
        .map(|run_dir| vec![run_dir.to_path_buf()])
        .unwrap_or_default()
}

pub(super) fn classify_release_evidence_json(
    kind: ReleaseEvidenceKind,
    value: &Value,
) -> ReleaseEvidenceStatus {
    let details = value
        .get("browser_details")
        .or_else(|| value.get("details"))
        .filter(|value| value.is_object());
    let text_status = text_field_deep(value, details, &["status"]);
    if let Some(status) = text_status.as_deref()
        && is_release_evidence_unavailable_status(status)
    {
        return ReleaseEvidenceStatus::Unavailable(evidence_unavailable_reason(
            value, details, status,
        ));
    }
    if let Some(status) =
        numeric_field_deep(value, details, &["http_status", "status", "status_code"])
        && status >= 400
    {
        return ReleaseEvidenceStatus::Failed(evidence_http_failure_reason(value, details, status));
    }
    if let Some(success) = bool_field_deep(
        value,
        details,
        &["ok", "success", "browser_success", "interaction_success"],
    ) && !success
    {
        return ReleaseEvidenceStatus::Failed(evidence_failure_reason(value, details));
    }
    if let Some(reason) = explicit_release_evidence_failure(kind, value, details) {
        return ReleaseEvidenceStatus::Failed(reason);
    }
    if let Some(status) = text_status.as_deref()
        && matches!(status, "failed" | "fail" | "error")
    {
        return ReleaseEvidenceStatus::Failed(evidence_failure_reason(value, details));
    }
    if let Some(kind_value) = text_field_deep(
        value,
        details,
        &["browser_failure_kind", "failure_kind", "error_kind"],
    ) && !kind_value.is_empty()
    {
        return ReleaseEvidenceStatus::Failed(kind_value);
    }
    if release_evidence_has_required_detail(kind, value, details) {
        return ReleaseEvidenceStatus::Passed;
    }
    let status_is_pass_like = text_status
        .as_deref()
        .is_some_and(|status| matches!(status, "ok" | "pass" | "passed" | "ready"));
    let success_is_true = bool_field_deep(
        value,
        details,
        &["ok", "success", "browser_success", "interaction_success"],
    ) == Some(true);
    let http_is_ok = numeric_field_deep(value, details, &["http_status", "status", "status_code"])
        .is_some_and(|status| (200..400).contains(&status));
    if success_is_true || status_is_pass_like || http_is_ok {
        return ReleaseEvidenceStatus::Unavailable(
            match kind {
                ReleaseEvidenceKind::BrowserReadiness => "browser_render_evidence_missing",
                ReleaseEvidenceKind::Interaction => "interaction_detail_missing",
            }
            .to_string(),
        );
    }
    ReleaseEvidenceStatus::Unavailable("evidence_inconclusive".to_string())
}

pub(super) fn explicit_release_evidence_failure(
    kind: ReleaseEvidenceKind,
    value: &Value,
    details: Option<&Value>,
) -> Option<String> {
    match kind {
        ReleaseEvidenceKind::BrowserReadiness => {
            if bool_field_deep(
                value,
                details,
                &["route_rendered", "rendered", "page_loaded", "dom_ready"],
            ) == Some(false)
            {
                return Some("browser_route_not_rendered".to_string());
            }
        }
        ReleaseEvidenceKind::Interaction => {
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
                if !transition_observed {
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

pub(super) fn release_evidence_has_required_detail(
    kind: ReleaseEvidenceKind,
    value: &Value,
    details: Option<&Value>,
) -> bool {
    match kind {
        ReleaseEvidenceKind::BrowserReadiness => {
            bool_field_deep(
                value,
                details,
                &["route_rendered", "rendered", "page_loaded", "dom_ready"],
            ) == Some(true)
        }
        ReleaseEvidenceKind::Interaction => {
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
            transition_observed && input_state_changed
        }
    }
}

pub(super) fn evidence_failure_reason(value: &Value, details: Option<&Value>) -> String {
    let text_reason = text_field_deep(
        value,
        details,
        &[
            "browser_failure_kind",
            "failure_kind",
            "error_kind",
            "status",
        ],
    );
    if text_reason
        .as_deref()
        .is_some_and(prefer_release_evidence_failure_kind_over_http)
    {
        return text_reason.unwrap();
    }
    if let Some(status) =
        numeric_field_deep(value, details, &["http_status", "status", "status_code"])
        && status >= 400
    {
        return format!("http_{status}");
    }
    text_reason.unwrap_or_else(|| "browser_check_failed".to_string())
}

pub(super) fn evidence_http_failure_reason(
    value: &Value,
    details: Option<&Value>,
    status: i64,
) -> String {
    text_field_deep(
        value,
        details,
        &["browser_failure_kind", "failure_kind", "error_kind"],
    )
    .filter(|reason| prefer_release_evidence_failure_kind_over_http(reason))
    .unwrap_or_else(|| format!("http_{status}"))
}

pub(super) fn evidence_unavailable_reason(
    value: &Value,
    details: Option<&Value>,
    status: &str,
) -> String {
    text_field_deep(
        value,
        details,
        &[
            "browser_failure_kind",
            "failure_kind",
            "error_kind",
            "reason",
        ],
    )
    .filter(|reason| !reason.is_empty())
    .unwrap_or_else(|| status.to_string())
}

pub(super) fn is_release_evidence_unavailable_status(status: &str) -> bool {
    matches!(
        status,
        "not_enabled" | "adapter_not_implemented" | "unavailable" | "skipped"
    ) || status.starts_with("unavailable:")
        || status == "browser_unavailable"
        || status.starts_with("browser_unavailable:")
        || status == "skipped_offline"
        || status == "skipped_unsupported_profile"
}

pub(super) fn prefer_release_evidence_failure_kind_over_http(reason: &str) -> bool {
    matches!(
        reason,
        "tailwind_dev_pipeline_failure"
            | "css_dev_pipeline_failure"
            | "nextjs_dev_pipeline_failure"
    )
}

pub(super) fn bool_field_deep(
    value: &Value,
    details: Option<&Value>,
    keys: &[&str],
) -> Option<bool> {
    bool_field(value, keys).or_else(|| details.and_then(|details| bool_field(details, keys)))
}

pub(super) fn bool_field(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_bool))
}

pub(super) fn numeric_field_deep(
    value: &Value,
    details: Option<&Value>,
    keys: &[&str],
) -> Option<i64> {
    numeric_field(value, keys).or_else(|| details.and_then(|details| numeric_field(details, keys)))
}

pub(super) fn numeric_field(value: &Value, keys: &[&str]) -> Option<i64> {
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

pub(super) fn text_field_deep(
    value: &Value,
    details: Option<&Value>,
    keys: &[&str],
) -> Option<String> {
    text_field(value, keys).or_else(|| details.and_then(|details| text_field(details, keys)))
}

pub(super) fn text_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(|text| text.trim().to_ascii_lowercase())
}

pub(super) fn string_array_field_contains_deep(
    value: &Value,
    details: Option<&Value>,
    key: &str,
    needle: &str,
) -> bool {
    string_array_field_contains(value, key, needle)
        || details.is_some_and(|details| string_array_field_contains(details, key, needle))
}

pub(super) fn string_array_field_contains(value: &Value, key: &str, needle: &str) -> bool {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|item| item == needle)
}

pub(super) fn release_gate_final_acceptance_status(
    release_gate: &ReleaseGateSummary,
) -> &'static str {
    match release_gate.status.as_str() {
        "pass" | "not_applicable" => "full_success",
        "partial" => "partial",
        "failed" => "incomplete",
        _ => "incomplete",
    }
}

pub(super) fn runtime_acceptance_status(
    runtime_ok: bool,
    report: Option<&crate::minimal_loop::evidence::RuntimeAcceptanceReport>,
) -> &'static str {
    match report {
        Some(report) if report.inconclusive => "inconclusive",
        Some(report) if !report.unverified_evidence.is_empty() => "partial",
        Some(_) if runtime_ok => "pass",
        Some(_) => "failed",
        None => "not_checked",
    }
}

pub(super) fn assurance_for_completion(
    profile: &str,
    required_capabilities: &[String],
) -> (&'static str, &'static str) {
    let profile = canonical_profile_name(profile);
    if profile == "data" {
        ("static", "data_profile_probe_not_run")
    } else if profile == "generic" {
        if required_capabilities
            .iter()
            .any(|capability| capability == GENERIC_INTERACTIVE_CONTRACT_CAPABILITY)
        {
            ("static", eval_events::GENERIC_STATIC_ASSURANCE_REASON)
        } else {
            ("reduced", eval_events::GENERIC_REDUCED_ASSURANCE_REASON)
        }
    } else {
        ("full", "")
    }
}

pub(super) fn earned_assurance_for_completion(
    profile: &str,
    required_capabilities: &[String],
    contract_bound: bool,
    final_acceptance_status: &str,
    release_gate: &ReleaseGateSummary,
    gate_telemetry: &AcceptanceGateTelemetry,
    profile_behavior_probe: Option<&ProfileBehaviorProbeReport>,
) -> (String, String) {
    let data_profile = canonical_profile_name(profile) == "data";
    if data_profile {
        let status = profile_behavior_probe.map(|report| report.status);
        if status != Some("pass") {
            let level = match status {
                Some("partial") => "partial",
                Some("failed") => "failed",
                _ => "static",
            };
            let reason = profile_behavior_probe
                .and_then(|report| report.reasons.first())
                .cloned()
                .unwrap_or_else(|| format!("data_assurance_{level}"));
            return (level.to_string(), reason);
        }
    }
    let (base_level, base_reason) = if data_profile {
        ("full", "")
    } else {
        assurance_for_completion(profile, required_capabilities)
    };
    if base_level != "full" {
        return (base_level.to_string(), base_reason.to_string());
    }
    if final_acceptance_status == "partial" || release_gate.status == "partial" {
        return (
            "partial".to_string(),
            release_gate
                .reasons
                .first()
                .cloned()
                .unwrap_or_else(|| "acceptance_partial".to_string()),
        );
    }
    if final_acceptance_status != "full_success" || release_gate.status == "failed" {
        return (
            "partial".to_string(),
            release_gate
                .reasons
                .first()
                .cloned()
                .unwrap_or_else(|| "acceptance_not_full_success".to_string()),
        );
    }
    if canonical_profile_name(profile).is_empty() {
        return (
            "partial".to_string(),
            "effective_profile_unknown".to_string(),
        );
    }
    if !contract_bound {
        return (
            "partial".to_string(),
            "completion_contract_not_bound".to_string(),
        );
    }
    if gate_telemetry.browser_readiness_applicable
        && gate_telemetry.browser_readiness_execution_status != "performed"
    {
        return (
            "partial".to_string(),
            format!(
                "browser_readiness_not_performed:{}",
                gate_telemetry.browser_readiness_execution_status
            ),
        );
    }
    if gate_telemetry.interaction_evidence_applicable
        && gate_telemetry.interaction_evidence_execution_status != "performed"
    {
        return (
            "partial".to_string(),
            format!(
                "interaction_evidence_not_performed:{}",
                gate_telemetry.interaction_evidence_execution_status
            ),
        );
    }
    ("full".to_string(), String::new())
}

pub(super) fn release_quality_completion_status(
    release_gate: &ReleaseGateSummary,
    final_acceptance_status: &str,
) -> &'static str {
    match release_gate.status.as_str() {
        "pass" | "not_applicable" => "release_ready",
        "partial" => "partial",
        "failed" => "failed",
        _ if final_acceptance_status == "partial" => "partial",
        _ => "failed",
    }
}

pub(super) fn release_gate_next_action(
    release_gate: &ReleaseGateSummary,
    final_acceptance_status: &str,
) -> &'static str {
    match release_gate.status.as_str() {
        "partial" => "collect_missing_release_evidence_or_continue_release_recovery",
        "failed" => "repair_release_gate_failure",
        _ if final_acceptance_status == "partial" => "collect_missing_final_acceptance_evidence",
        _ => "none",
    }
}

pub(super) fn release_recovery_needed(
    release_gate: &ReleaseGateSummary,
    final_acceptance_status: &str,
) -> bool {
    matches!(release_gate.status.as_str(), "partial" | "failed")
        || matches!(final_acceptance_status, "partial" | "failed" | "incomplete")
}

pub(super) fn release_recovery_acceptance_layer(
    release_gate: &ReleaseGateSummary,
    final_acceptance_status: &str,
) -> &'static str {
    match release_gate.status.as_str() {
        "partial" | "failed" => "release_gate",
        _ if final_acceptance_status == "partial" => "final_acceptance_partial",
        _ => "final_acceptance",
    }
}
