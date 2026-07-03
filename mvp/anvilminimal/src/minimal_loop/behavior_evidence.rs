use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::minimal_loop::evidence::{RuntimeAcceptanceReport, refresh_runtime_acceptance_report};

const INTERACTION_EVIDENCE_NAMES: &[&str] = &[
    "interaction-evidence.json",
    "interaction.json",
    "browser-interaction.json",
];

const SURFACE_AND_START_KEYS: &[&str] = &[
    "interactive_ui_source_evidence",
    "visible_interactive_surface_evidence",
    "non_static_screen_evidence",
];

const INPUT_STATE_KEYS: &[&str] = &["user_input_handler_evidence", "stateful_update_evidence"];

const WEAK_BEHAVIOR_CORROBORATED_KEYS: &[&str] = &[
    "score_or_progression_evidence",
    "challenge_or_adversary_evidence",
    "failure_or_collision_evidence",
    "restart_or_recoverable_state_evidence",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceArbitrationRecord {
    pub static_tier: String,
    pub behavioral_observation: String,
    pub decided_by: String,
    pub final_tier: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceArbitrationReport {
    pub summary: String,
    pub records: BTreeMap<String, EvidenceArbitrationRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BehaviorObservation {
    ok: bool,
    failure_kind: String,
    stage: String,
    steps: BTreeSet<String>,
    surface_visible: bool,
    start_transition: bool,
    input_state_change: bool,
    recovery_transition: RecoveryTransition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryTransition {
    Observed,
    NotObserved,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BehavioralDecision {
    Pass(&'static str),
    Fail(&'static str),
    Static(&'static str),
}

pub fn arbitrate_final_acceptance(
    report: &mut RuntimeAcceptanceReport,
    root: &Path,
    extra_dirs: &[PathBuf],
    required_capabilities: &[String],
    required_evidence: &[String],
    required_obligations: &[String],
) -> EvidenceArbitrationReport {
    let static_tiers = report.evidence_tiers.clone();
    let observation = read_behavior_observation(root, extra_dirs);
    let mut records = BTreeMap::new();

    let Some(observation) = observation else {
        for (key, static_tier) in static_tiers {
            records.insert(
                key,
                EvidenceArbitrationRecord {
                    final_tier: static_tier.clone(),
                    static_tier,
                    behavioral_observation: "probe_unavailable".to_string(),
                    decided_by: "static".to_string(),
                },
            );
        }
        return EvidenceArbitrationReport {
            summary: "static (probe unavailable)".to_string(),
            records,
        };
    };

    if observation.infrastructure_failure() {
        for (key, static_tier) in static_tiers {
            records.insert(
                key,
                EvidenceArbitrationRecord {
                    final_tier: static_tier.clone(),
                    static_tier,
                    behavioral_observation: format!(
                        "probe_infrastructure_failure:{}",
                        observation.failure_kind
                    ),
                    decided_by: "static".to_string(),
                },
            );
        }
        return EvidenceArbitrationReport {
            summary: format!(
                "static (probe infrastructure failure: {})",
                observation.failure_kind
            ),
            records,
        };
    }

    for (key, static_tier) in &static_tiers {
        let decision = behavioral_decision(key, static_tier, &observation);
        match decision {
            BehavioralDecision::Pass(observed) => {
                let final_tier = if static_tier == "weak" {
                    "weak_behavior_corroborated"
                } else {
                    "strong"
                };
                satisfy_evidence(report, key, final_tier);
                records.insert(
                    key.clone(),
                    EvidenceArbitrationRecord {
                        static_tier: static_tier.clone(),
                        behavioral_observation: observed.to_string(),
                        decided_by: "behavioral".to_string(),
                        final_tier: final_tier.to_string(),
                    },
                );
            }
            BehavioralDecision::Fail(observed) => {
                fail_evidence(report, key);
                records.insert(
                    key.clone(),
                    EvidenceArbitrationRecord {
                        static_tier: static_tier.clone(),
                        behavioral_observation: observed.to_string(),
                        decided_by: "behavioral".to_string(),
                        final_tier: "absent".to_string(),
                    },
                );
            }
            BehavioralDecision::Static(observed) => {
                records.insert(
                    key.clone(),
                    EvidenceArbitrationRecord {
                        final_tier: report
                            .evidence_tiers
                            .get(key)
                            .cloned()
                            .unwrap_or_else(|| static_tier.clone()),
                        static_tier: static_tier.clone(),
                        behavioral_observation: observed.to_string(),
                        decided_by: "static".to_string(),
                    },
                );
            }
        }
    }

    refresh_runtime_acceptance_report(
        report,
        required_capabilities,
        required_evidence,
        required_obligations,
    );
    EvidenceArbitrationReport {
        summary: if observation.ok {
            "behavioral (probe ok)".to_string()
        } else if observation.failure_kind.is_empty() {
            "behavioral (app failure)".to_string()
        } else {
            format!("behavioral (app failure: {})", observation.failure_kind)
        },
        records,
    }
}

fn behavioral_decision(
    key: &str,
    static_tier: &str,
    observation: &BehaviorObservation,
) -> BehavioralDecision {
    if SURFACE_AND_START_KEYS.contains(&key) {
        return if observation.surface_visible && observation.start_transition {
            BehavioralDecision::Pass("surface_visible+start_transition")
        } else if !observation.surface_visible {
            BehavioralDecision::Fail("surface_visible_missing")
        } else {
            BehavioralDecision::Fail("start_transition_missing")
        };
    }
    if INPUT_STATE_KEYS.contains(&key) {
        return if observation.input_state_change {
            BehavioralDecision::Pass("input_state_change")
        } else {
            BehavioralDecision::Fail("input_state_change_missing")
        };
    }
    if key == "restart_or_recoverable_state_evidence" {
        return match observation.recovery_transition {
            RecoveryTransition::Observed => BehavioralDecision::Pass("recovery_transition"),
            RecoveryTransition::NotObserved | RecoveryTransition::Unknown => {
                if observation.start_transition {
                    weak_corroboration_or_static(key, static_tier, observation)
                } else {
                    BehavioralDecision::Fail("start_transition_missing")
                }
            }
        };
    }
    weak_corroboration_or_static(key, static_tier, observation)
}

fn weak_corroboration_or_static(
    key: &str,
    static_tier: &str,
    observation: &BehaviorObservation,
) -> BehavioralDecision {
    if observation.ok && static_tier == "weak" && WEAK_BEHAVIOR_CORROBORATED_KEYS.contains(&key) {
        BehavioralDecision::Pass("probe_ok+weak_static_signal")
    } else {
        BehavioralDecision::Static(if observation.ok {
            "probe_ok_not_mapped"
        } else {
            "probe_failed"
        })
    }
}

fn satisfy_evidence(report: &mut RuntimeAcceptanceReport, key: &str, tier: &str) {
    report.missing_evidence.retain(|evidence| evidence != key);
    report.weak_evidence.retain(|evidence| {
        !evidence
            .strip_prefix("weak_source_evidence:")
            .is_some_and(|rest| rest.starts_with(key) && rest[key.len()..].starts_with(':'))
    });
    report
        .evidence_tiers
        .insert(key.to_string(), tier.to_string());
}

fn fail_evidence(report: &mut RuntimeAcceptanceReport, key: &str) {
    if !report
        .missing_evidence
        .iter()
        .any(|evidence| evidence == key)
    {
        report.missing_evidence.push(key.to_string());
    }
    report.weak_evidence.retain(|evidence| {
        !evidence
            .strip_prefix("weak_source_evidence:")
            .is_some_and(|rest| rest.starts_with(key) && rest[key.len()..].starts_with(':'))
    });
    report
        .evidence_tiers
        .insert(key.to_string(), "absent".to_string());
}

fn read_behavior_observation(root: &Path, extra_dirs: &[PathBuf]) -> Option<BehaviorObservation> {
    for path in interaction_evidence_candidate_paths(root, extra_dirs) {
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(path).ok()?;
        let value = serde_json::from_str::<Value>(&text).ok()?;
        if !value.is_object() || evidence_status_unavailable(&value) {
            return None;
        }
        if let Some(observation) = BehaviorObservation::from_value(&value) {
            return Some(observation);
        }
    }
    None
}

fn interaction_evidence_candidate_paths(root: &Path, extra_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs = extra_dirs.to_vec();
    dirs.push(root.join(".anvil").join("evidence"));
    dirs.push(root.join(".anvil"));
    dirs.push(root.to_path_buf());
    let mut out = Vec::new();
    for dir in dirs {
        for name in INTERACTION_EVIDENCE_NAMES {
            let path = dir.join(name);
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

fn evidence_status_unavailable(value: &Value) -> bool {
    text_field_deep(value, &["status"]).is_some_and(|status| {
        matches!(
            status.as_str(),
            "not_enabled"
                | "adapter_not_implemented"
                | "unavailable"
                | "skipped"
                | "skipped_offline"
                | "skipped_unsupported_profile"
        )
    })
}

impl BehaviorObservation {
    fn from_value(value: &Value) -> Option<Self> {
        let steps = steps_from_value(value);
        let probe_shaped = !steps.is_empty()
            || value.get("probe").is_some()
            || text_field_deep(value, &["before_marker"]).is_some()
            || text_field_deep(value, &["after_marker"]).is_some()
            || text_field_deep(value, &["stage"]).is_some()
            || text_field_deep(value, &["failure_kind", "browser_failure_kind"]).is_some();
        if !probe_shaped {
            return None;
        }
        let explicit_ok = bool_field_deep(
            value,
            &["ok", "success", "browser_success", "interaction_success"],
        );
        let status_passed = text_field_deep(value, &["status"])
            .is_some_and(|status| matches!(status.as_str(), "ok" | "pass" | "passed" | "ready"));
        let ok = explicit_ok.unwrap_or(status_passed);
        if explicit_ok.is_none() && !status_passed && steps.is_empty() {
            return None;
        }
        let failure_kind = text_field_deep(
            value,
            &["browser_failure_kind", "failure_kind", "error_kind"],
        )
        .unwrap_or_else(|| {
            if ok {
                String::new()
            } else {
                "browser_interaction_failed".to_string()
            }
        });
        let stage = text_field_deep(value, &["stage"]).unwrap_or_default();
        let surface_visible = steps.contains("surface_visible")
            || bool_field_deep(value, &["surface_visible", "interactive_surface"]) == Some(true);
        let marker_changed = marker_changed(value, "before_marker", "after_marker");
        let start_transition = steps.contains("start_transition")
            || bool_field_deep(value, &["start_transition"]) == Some(true)
            || marker_changed;
        let input_state_change = input_state_changed(value, &steps, ok);
        let recovery_transition = recovery_transition(value, &steps);
        Some(Self {
            ok,
            failure_kind,
            stage,
            steps,
            surface_visible,
            start_transition,
            input_state_change,
            recovery_transition,
        })
    }

    fn infrastructure_failure(&self) -> bool {
        !self.ok
            && (self.failure_kind.starts_with("probe_dependency_missing")
                || self.failure_kind.starts_with("probe_infrastructure_failed")
                || (matches!(
                    self.stage.as_str(),
                    "resolving" | "launching" | "navigating"
                ) && !self.surface_visible))
    }
}

fn steps_from_value(value: &Value) -> BTreeSet<String> {
    value
        .get("steps")
        .or_else(|| {
            value
                .get("details")
                .and_then(|details| details.get("steps"))
        })
        .or_else(|| {
            value
                .get("browser_details")
                .and_then(|details| details.get("steps"))
        })
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

fn input_state_changed(value: &Value, steps: &BTreeSet<String>, ok: bool) -> bool {
    if bool_field_deep(
        value,
        &[
            "input_state_change",
            "state_changed",
            "visible_state_changed",
        ],
    ) == Some(false)
    {
        return false;
    }
    steps.contains("input_state_change")
        || bool_field_deep(
            value,
            &[
                "input_state_change",
                "state_changed",
                "visible_state_changed",
            ],
        ) == Some(true)
        || marker_changed(value, "input_before_marker", "input_after_marker")
        || marker_changed(value, "control_before_marker", "control_after_marker")
        || (ok
            && steps.contains("control_input_dispatched")
            && bool_field_deep(value, &["input_event_observed"]) == Some(true))
}

fn recovery_transition(value: &Value, steps: &BTreeSet<String>) -> RecoveryTransition {
    if steps.contains("recovery_transition")
        || bool_field_deep(value, &["recovery_transition"]) == Some(true)
    {
        return RecoveryTransition::Observed;
    }
    if steps.contains("recovery_transition:not_observed")
        || bool_field_deep(value, &["recovery_transition"]) == Some(false)
        || text_field_deep(value, &["recovery_transition_status"]).as_deref()
            == Some("not_observed")
    {
        return RecoveryTransition::NotObserved;
    }
    RecoveryTransition::Unknown
}

fn marker_changed(value: &Value, before_key: &str, after_key: &str) -> bool {
    let before = text_field_deep(value, &[before_key]);
    let after = text_field_deep(value, &[after_key]);
    before
        .zip(after)
        .is_some_and(|(before, after)| !before.is_empty() && before != after)
}

fn bool_field_deep(value: &Value, names: &[&str]) -> Option<bool> {
    for scope in value_scopes(value) {
        for name in names {
            if let Some(found) = scope.get(*name).and_then(Value::as_bool) {
                return Some(found);
            }
        }
    }
    None
}

fn text_field_deep(value: &Value, names: &[&str]) -> Option<String> {
    for scope in value_scopes(value) {
        for name in names {
            if let Some(found) = scope.get(*name).and_then(Value::as_str) {
                return Some(found.trim().to_string());
            }
        }
    }
    None
}

fn value_scopes(value: &Value) -> Vec<&Value> {
    let mut scopes = vec![value];
    if let Some(details) = value.get("details").filter(|details| details.is_object()) {
        scopes.push(details);
    }
    if let Some(details) = value
        .get("browser_details")
        .filter(|details| details.is_object())
    {
        scopes.push(details);
    }
    scopes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minimal_loop::evidence::verify_runtime_acceptance_with_browser_dirs_and_hints;
    use serde_json::json;

    fn write_page(root: &Path, content: &str) {
        let path = root.join("src/app/page.tsx");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn weak_restart_and_collision_page() -> &'static str {
        r#""use client";
import { useState } from "react";
export default function Page() {
  const [score, setScore] = useState(0);
  const [mode, setMode] = useState("ready");
  const initGame = () => setMode("ready");
  const fire = () => setScore((value) => value + 1);
  return <main><button onClick={fire}>Start</button><button onClick={initGame}>Restart</button><canvas />score enemy collision restart {score}{mode}</main>;
}
"#
    }

    fn strong_interactive_page() -> &'static str {
        r#""use client";
import { useEffect, useState } from "react";
export default function Page() {
  const [score, setScore] = useState(0);
  const [gameState, setGameState] = useState("ready");
  const [enemies, setEnemies] = useState([{ x: 1 }]);
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") setScore((value) => value + 1);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
  const restart = () => { setScore(0); setEnemies([{ x: 1 }]); setGameState("playing"); };
  const collision = enemies.some((enemy) => enemy.x > 0);
  if (collision && gameState === "playing") setGameState("gameover");
  return <main><button onClick={() => setGameState("playing")}>Start</button><button onClick={restart}>Restart</button><canvas />score enemy collision {score}</main>;
}
"#
    }

    fn write_interaction(root: &Path, value: Value) -> PathBuf {
        let run_dir = root.join(".anvil/runs/test");
        std::fs::create_dir_all(&run_dir).unwrap();
        let path = run_dir.join("browser-interaction.json");
        std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
        path
    }

    fn report_for(root: &Path, required_evidence: &[&str]) -> RuntimeAcceptanceReport {
        verify_runtime_acceptance_with_browser_dirs_and_hints(
            root,
            &["src/app/page.tsx".to_string()],
            &[],
            &[],
            &required_evidence
                .iter()
                .map(|evidence| evidence.to_string())
                .collect::<Vec<_>>(),
            &[],
            &[],
            &[root.join(".anvil/runs/test")],
            &[],
        )
    }

    fn arbitrate(root: &Path, report: &mut RuntimeAcceptanceReport, required_evidence: &[&str]) {
        arbitrate_final_acceptance(
            report,
            root,
            &[root.join(".anvil/runs/test")],
            &[],
            &required_evidence
                .iter()
                .map(|evidence| evidence.to_string())
                .collect::<Vec<_>>(),
            &[],
        );
    }

    #[test]
    fn probe_ok_corrobates_weak_restart_and_collision() {
        let dir = tempfile::tempdir().unwrap();
        write_page(dir.path(), weak_restart_and_collision_page());
        write_interaction(
            dir.path(),
            json!({
                "ok": true,
                "status": "passed",
                "interaction_success": true,
                "interaction_performed": true,
                "input_event_observed": true,
                "state_changed": true,
                "steps": [
                    "surface_visible",
                    "start_transition",
                    "control_input_dispatched",
                    "input_state_change",
                    "recovery_transition"
                ],
                "before_marker": "menu",
                "after_marker": "running",
                "input_before_marker": "running",
                "input_after_marker": "moved"
            }),
        );
        let required = [
            "visible_interactive_surface_evidence",
            "interactive_ui_source_evidence",
            "non_static_screen_evidence",
            "user_input_handler_evidence",
            "stateful_update_evidence",
            "restart_or_recoverable_state_evidence",
            "failure_or_collision_evidence",
        ];
        let mut report = report_for(dir.path(), &required);
        assert!(!report.passed, "{report:?}");
        arbitrate(dir.path(), &mut report, &required);
        assert!(report.passed, "{report:?}");
        assert_eq!(
            report
                .evidence_tiers
                .get("restart_or_recoverable_state_evidence")
                .map(String::as_str),
            Some("weak_behavior_corroborated")
        );
        assert_eq!(
            report
                .evidence_tiers
                .get("failure_or_collision_evidence")
                .map(String::as_str),
            Some("weak_behavior_corroborated")
        );
    }

    #[test]
    fn start_transition_failure_overrides_static_strong() {
        let dir = tempfile::tempdir().unwrap();
        write_page(dir.path(), strong_interactive_page());
        write_interaction(
            dir.path(),
            json!({
                "ok": false,
                "status": "failed",
                "interaction_success": false,
                "input_event_observed": true,
                "state_changed": false,
                "steps": ["surface_visible", "control_input_dispatched"],
                "stage": "observing",
                "before_marker": "same",
                "after_marker": "same",
                "failure_kind": "start_transition_missing"
            }),
        );
        let required = [
            "interactive_ui_source_evidence",
            "visible_interactive_surface_evidence",
            "non_static_screen_evidence",
            "restart_or_recoverable_state_evidence",
        ];
        let mut report = report_for(dir.path(), &required);
        assert_eq!(
            report
                .evidence_tiers
                .get("restart_or_recoverable_state_evidence")
                .map(String::as_str),
            Some("strong"),
            "{report:?}"
        );
        arbitrate(dir.path(), &mut report, &required);
        assert!(!report.passed, "{report:?}");
        assert!(
            report
                .missing_evidence
                .contains(&"restart_or_recoverable_state_evidence".to_string())
        );
        assert_eq!(
            report
                .evidence_tiers
                .get("restart_or_recoverable_state_evidence")
                .map(String::as_str),
            Some("absent")
        );
    }

    #[test]
    fn probe_unavailable_keeps_static_outcome() {
        let dir = tempfile::tempdir().unwrap();
        write_page(dir.path(), weak_restart_and_collision_page());
        let required = ["restart_or_recoverable_state_evidence"];
        let mut report = report_for(dir.path(), &required);
        let before = report.clone();
        let arbitration = arbitrate_final_acceptance(
            &mut report,
            dir.path(),
            &[dir.path().join(".anvil/runs/test")],
            &[],
            &required
                .iter()
                .map(|evidence| evidence.to_string())
                .collect::<Vec<_>>(),
            &[],
        );
        assert_eq!(report, before);
        assert_eq!(arbitration.summary, "static (probe unavailable)");
    }

    #[test]
    fn infrastructure_failure_keeps_static_tiers() {
        let dir = tempfile::tempdir().unwrap();
        write_page(dir.path(), strong_interactive_page());
        write_interaction(
            dir.path(),
            json!({
                "ok": false,
                "status": "failed",
                "interaction_success": false,
                "stage": "resolving",
                "steps": [],
                "failure_kind": "probe_dependency_missing:playwright_module_missing",
                "remediation": "install playwright or set ANVIL_PLAYWRIGHT_DIR"
            }),
        );
        let required = [
            "interactive_ui_source_evidence",
            "visible_interactive_surface_evidence",
            "non_static_screen_evidence",
            "restart_or_recoverable_state_evidence",
        ];
        let mut report = report_for(dir.path(), &required);
        let before = report.clone();
        let arbitration = arbitrate_final_acceptance(
            &mut report,
            dir.path(),
            &[dir.path().join(".anvil/runs/test")],
            &[],
            &required
                .iter()
                .map(|evidence| evidence.to_string())
                .collect::<Vec<_>>(),
            &[],
        );
        assert_eq!(report, before);
        assert_eq!(
            arbitration.summary,
            "static (probe infrastructure failure: probe_dependency_missing:playwright_module_missing)"
        );
        assert_eq!(
            arbitration
                .records
                .get("interactive_ui_source_evidence")
                .map(|record| record.decided_by.as_str()),
            Some("static")
        );
    }

    #[test]
    fn static_absent_non_probe_key_still_fails_with_probe_ok() {
        let dir = tempfile::tempdir().unwrap();
        write_page(
            dir.path(),
            r#""use client";
import { useState } from "react";
export default function Page() {
  const [score, setScore] = useState(0);
  return <main><button onClick={() => setScore(score + 1)}>Start</button><canvas />score {score}</main>;
}
"#,
        );
        write_interaction(
            dir.path(),
            json!({
                "ok": true,
                "status": "passed",
                "interaction_success": true,
                "interaction_performed": true,
                "input_event_observed": true,
                "state_changed": true,
                "steps": [
                    "surface_visible",
                    "start_transition",
                    "control_input_dispatched",
                    "input_state_change"
                ],
                "before_marker": "menu",
                "after_marker": "running"
            }),
        );
        let required = ["challenge_or_adversary_evidence"];
        let mut report = report_for(dir.path(), &required);
        arbitrate(dir.path(), &mut report, &required);
        assert!(!report.passed, "{report:?}");
        assert_eq!(
            report
                .evidence_tiers
                .get("challenge_or_adversary_evidence")
                .map(String::as_str),
            Some("absent")
        );
    }
}
