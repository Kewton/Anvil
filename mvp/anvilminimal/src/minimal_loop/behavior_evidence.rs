use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::minimal_loop::evidence::{RuntimeAcceptanceReport, refresh_runtime_acceptance_report};

const INTERACTION_EVIDENCE_NAMES: &[&str] = &[
    "browser-interaction.json",
    "interaction-evidence.json",
    "interaction.json",
];

const SURFACE_KEYS: &[&str] = &[
    "interactive_ui_source_evidence",
    "visible_interactive_surface_evidence",
    "non_static_screen_evidence",
];

const INPUT_STATE_KEYS: &[&str] = &["user_input_handler_evidence", "stateful_update_evidence"];

const LIVE_PREVIEW_KEYS: &[&str] = &["live_preview_evidence", "requested_content_evidence"];

const PROBE_REQUIRED_BEHAVIOR_KEYS: &[&str] = &[
    "interactive_ui_source_evidence",
    "visible_interactive_surface_evidence",
    "non_static_screen_evidence",
    "user_input_handler_evidence",
    "stateful_update_evidence",
    "restart_or_recoverable_state_evidence",
    "live_preview_evidence",
];

const DEEP_BEHAVIOR_KEYS: &[&str] = &[
    "score_or_progression_evidence",
    "challenge_or_adversary_evidence",
    "failure_or_collision_evidence",
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
    input_state_evaluated_after_start: bool,
    input_state_change: bool,
    text_entry: String,
    token_echoed: bool,
    text_input_state_change: bool,
    recovery_transition: RecoveryTransition,
    persistence_after_reload: PersistenceAfterReload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryTransition {
    Observed,
    NotObserved,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PersistenceAfterReload {
    Preserved,
    Reset,
    NotEvaluated,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BehavioralDecision {
    Pass(&'static str),
    Fail(&'static str),
    Unverified(&'static str),
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
            let unverified = mark_unverified_if_probe_required_weak(
                report,
                &key,
                &static_tier,
                "probe_unavailable",
            );
            let final_tier = if unverified {
                "unverified:probe_unavailable".to_string()
            } else {
                static_tier.clone()
            };
            records.insert(
                key,
                EvidenceArbitrationRecord {
                    final_tier,
                    static_tier,
                    behavioral_observation: "unverified:probe_unavailable".to_string(),
                    decided_by: if unverified {
                        "probe_required"
                    } else {
                        "static"
                    }
                    .to_string(),
                },
            );
        }
        refresh_runtime_acceptance_report(
            report,
            required_capabilities,
            required_evidence,
            required_obligations,
        );
        return EvidenceArbitrationReport {
            summary: "partial (probe unavailable)".to_string(),
            records,
        };
    };

    if observation.infrastructure_failure() {
        let probe_reason = observation.failure_kind.as_str();
        for (key, static_tier) in static_tiers {
            let unverified =
                mark_unverified_if_probe_required_weak(report, &key, &static_tier, probe_reason);
            let final_tier = if unverified {
                format!("unverified:{probe_reason}")
            } else {
                static_tier.clone()
            };
            records.insert(
                key,
                EvidenceArbitrationRecord {
                    final_tier,
                    static_tier,
                    behavioral_observation: format!(
                        "probe_infrastructure_failure:{}",
                        observation.failure_kind
                    ),
                    decided_by: if unverified {
                        "probe_required"
                    } else {
                        "static"
                    }
                    .to_string(),
                },
            );
        }
        refresh_runtime_acceptance_report(
            report,
            required_capabilities,
            required_evidence,
            required_obligations,
        );
        return EvidenceArbitrationReport {
            summary: format!(
                "partial (probe infrastructure failure: {})",
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
            BehavioralDecision::Unverified(observed) => {
                mark_unverified_evidence(report, key, observed);
                records.insert(
                    key.clone(),
                    EvidenceArbitrationRecord {
                        static_tier: static_tier.clone(),
                        behavioral_observation: observed.to_string(),
                        decided_by: "behavioral".to_string(),
                        final_tier: format!("unverified:{observed}"),
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
    if SURFACE_KEYS.contains(&key) {
        return if observation.surface_visible {
            BehavioralDecision::Pass("surface_visible")
        } else {
            BehavioralDecision::Fail("surface_visible_missing")
        };
    }
    if INPUT_STATE_KEYS.contains(&key) {
        return if observation.text_input_state_change {
            BehavioralDecision::Pass("text_input_state_change")
        } else if observation.input_state_change {
            BehavioralDecision::Pass("input_state_change")
        } else if observation.failure_kind == "text_input_state_change_missing" {
            BehavioralDecision::Fail("text_input_state_change_missing")
        } else if observation.start_transition && !observation.input_state_evaluated_after_start {
            BehavioralDecision::Fail("input_state_change_not_evaluated_after_start")
        } else {
            BehavioralDecision::Fail("input_state_change_missing_after_start")
        };
    }
    if LIVE_PREVIEW_KEYS.contains(&key) {
        return if observation.token_echoed {
            BehavioralDecision::Pass("token_echoed")
        } else if observation.text_entry == "not_applicable" {
            BehavioralDecision::Fail("text_entry_not_applicable")
        } else {
            BehavioralDecision::Fail("token_echo_missing")
        };
    }
    if key == "restart_or_recoverable_state_evidence" {
        return match observation.recovery_transition {
            RecoveryTransition::Observed => BehavioralDecision::Pass("recovery_transition"),
            RecoveryTransition::NotObserved
                if observation.start_transition && observation.input_state_change =>
            {
                BehavioralDecision::Fail("not_observed_by_probe")
            }
            RecoveryTransition::NotObserved if observation.start_transition => {
                BehavioralDecision::Fail("input_state_change_missing_after_start")
            }
            RecoveryTransition::NotObserved => BehavioralDecision::Fail("start_transition_missing"),
            RecoveryTransition::Unknown => {
                if observation.ok && static_tier == "strong" {
                    BehavioralDecision::Static("strong_static_required")
                } else if observation.start_transition && observation.input_state_change {
                    BehavioralDecision::Fail("not_observed_by_probe")
                } else if observation.start_transition {
                    BehavioralDecision::Fail("input_state_change_missing_after_start")
                } else {
                    BehavioralDecision::Fail("start_transition_missing")
                }
            }
        };
    }
    if key == "persistence_evidence" {
        return match observation.persistence_after_reload {
            PersistenceAfterReload::Preserved => BehavioralDecision::Pass("preserved_after_reload"),
            PersistenceAfterReload::Reset => BehavioralDecision::Fail("reset_after_reload"),
            PersistenceAfterReload::NotEvaluated => BehavioralDecision::Unverified("not_evaluated"),
            PersistenceAfterReload::Unknown => BehavioralDecision::Static(if observation.ok {
                "probe_ok_not_mapped"
            } else {
                "probe_failed"
            }),
        };
    }
    if DEEP_BEHAVIOR_KEYS.contains(&key) {
        return if static_tier == "weak" {
            BehavioralDecision::Fail("not_observed_by_probe")
        } else {
            BehavioralDecision::Static(if observation.ok {
                "strong_static_required"
            } else {
                "probe_failed"
            })
        };
    }
    BehavioralDecision::Static(if observation.ok {
        "probe_ok_not_mapped"
    } else {
        "probe_failed"
    })
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

fn mark_unverified_evidence(report: &mut RuntimeAcceptanceReport, key: &str, reason: &str) {
    report.missing_evidence.retain(|evidence| evidence != key);
    report.weak_evidence.retain(|evidence| {
        !evidence
            .strip_prefix("weak_source_evidence:")
            .is_some_and(|rest| rest.starts_with(key) && rest[key.len()..].starts_with(':'))
    });
    let classified = format!("{key}:unverified:{reason}");
    if !report
        .unverified_evidence
        .iter()
        .any(|evidence| evidence == &classified)
    {
        report.unverified_evidence.push(classified);
    }
    report
        .evidence_tiers
        .insert(key.to_string(), format!("unverified:{reason}"));
}

fn mark_unverified_if_probe_required_weak(
    report: &mut RuntimeAcceptanceReport,
    key: &str,
    static_tier: &str,
    probe_reason: &str,
) -> bool {
    if static_tier != "weak" || !PROBE_REQUIRED_BEHAVIOR_KEYS.contains(&key) {
        return false;
    }
    mark_unverified_evidence(report, key, probe_reason);
    true
}

fn read_behavior_observation(root: &Path, extra_dirs: &[PathBuf]) -> Option<BehaviorObservation> {
    for path in interaction_evidence_candidate_paths(root, extra_dirs) {
        if !path.is_file() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if !value.is_object() || evidence_status_unavailable(&value) {
            continue;
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
        let input_state_evaluated_after_start = bool_field_deep(
            value,
            &[
                "input_state_evaluated_after_start",
                "input_evaluated_after_start",
            ],
        ) == Some(true)
            || steps.contains("input_state_evaluated_after_start")
            || (start_transition
                && text_field_deep(value, &["input_before_marker"])
                    .is_some_and(|marker| !marker.is_empty())
                && text_field_deep(value, &["input_after_marker"])
                    .is_some_and(|marker| !marker.is_empty()));
        let input_state_change = input_state_changed(value, &steps, ok);
        let text_entry = text_field_deep(value, &["text_entry"]).unwrap_or_default();
        let token_echoed = bool_field_deep(value, &["token_echoed"]) == Some(true)
            || steps.contains("token_echoed");
        let text_input_state_change = bool_field_deep(value, &["text_input_state_change"])
            == Some(true)
            || steps.contains("text_input_state_change");
        let recovery_transition = recovery_transition(value, &steps);
        let persistence_after_reload = persistence_after_reload(value, &steps);
        Some(Self {
            ok,
            failure_kind,
            stage,
            steps,
            surface_visible,
            start_transition,
            input_state_evaluated_after_start,
            input_state_change,
            text_entry,
            token_echoed,
            text_input_state_change,
            recovery_transition,
            persistence_after_reload,
        })
    }

    fn infrastructure_failure(&self) -> bool {
        if self.failure_kind == "app_route_unresponsive"
            || self.failure_kind.starts_with("probe_navigation_failed")
        {
            return false;
        }
        !self.ok
            && (self.failure_kind.starts_with("probe_dependency_missing")
                || self.failure_kind.starts_with("probe_infrastructure_failed")
                || (matches!(self.stage.as_str(), "resolving" | "launching")
                    && !self.surface_visible))
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

fn input_state_changed(value: &Value, steps: &BTreeSet<String>, _ok: bool) -> bool {
    if bool_field_deep(value, &["text_input_state_change"]) == Some(true)
        || steps.contains("text_input_state_change")
    {
        return true;
    }
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
                "text_input_state_change",
            ],
        ) == Some(true)
        || marker_changed(value, "input_before_marker", "input_after_marker")
        || marker_changed(value, "control_before_marker", "control_after_marker")
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

fn persistence_after_reload(value: &Value, steps: &BTreeSet<String>) -> PersistenceAfterReload {
    match text_field_deep(value, &["persistence_after_reload"]).as_deref() {
        Some("preserved") => return PersistenceAfterReload::Preserved,
        Some("reset") => return PersistenceAfterReload::Reset,
        Some("not_evaluated") => return PersistenceAfterReload::NotEvaluated,
        Some(_) => {}
        None => {}
    }
    if steps.contains("persistence_reload:preserved") {
        PersistenceAfterReload::Preserved
    } else if steps.contains("persistence_reload:reset") {
        PersistenceAfterReload::Reset
    } else if steps.contains("persistence_reload:not_evaluated") {
        PersistenceAfterReload::NotEvaluated
    } else {
        PersistenceAfterReload::Unknown
    }
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

    fn todo_persistence_page() -> &'static str {
        r#""use client";
import { useEffect, useState } from "react";
export default function Page() {
  const [items, setItems] = useState<string[]>(() => {
    if (typeof window === "undefined") return [];
    return JSON.parse(localStorage.getItem("todos") || "[]");
  });
  const add = () => setItems((value) => [...value, "anvil probe input"]);
  useEffect(() => {
    localStorage.setItem("todos", JSON.stringify(items));
  }, [items]);
  return <main data-anvil-state={JSON.stringify({ items })}>
    <button data-anvil-action="primary" onClick={add}>Add</button>
    <p>{items.join(",")}</p>
  </main>;
}
"#
    }

    fn notes_live_preview_page() -> &'static str {
        r#""use client";
import { useState } from "react";
export default function Page() {
  const [draft, setDraft] = useState("");
  return <main data-anvil-state={JSON.stringify({ draft })}>
    <textarea data-anvil-action="input" value={draft} onChange={(event) => setDraft(event.target.value)} />
    <section aria-label="Preview">{draft}</section>
  </main>;
}
"#
    }

    fn todo_text_entry_page() -> &'static str {
        r#""use client";
import { useState } from "react";
export default function Page() {
  const [draft, setDraft] = useState("");
  const [items, setItems] = useState<string[]>([]);
  const add = () => setItems((value) => draft ? [...value, draft] : value);
  return <main data-anvil-state={JSON.stringify({ draft, items })}>
    <input data-anvil-action="input" value={draft} onChange={(event) => setDraft(event.target.value)} />
    <button data-anvil-action="primary" onClick={add}>Add</button>
    <ul>{items.map((item) => <li key={item}>{item}</li>)}</ul>
  </main>;
}
"#
    }

    fn write_cross_file_weak_restart_fixture(root: &Path) {
        write_page(
            root,
            r#""use client";
import { useRef, useState } from "react";
import { GameEngine } from "./gameEngine";
export default function Page() {
  const engineRef = useRef(new GameEngine());
  const [screen, setScreen] = useState("gameOver");
  const startGame = () => {
    engineRef.current?.reset();
    setScreen("playing");
  };
  return <main><button onClick={startGame}>Restart</button><canvas />score enemy collision {screen}</main>;
}
"#,
        );
        let engine = root.join("src/app/gameEngine.ts");
        std::fs::write(
            engine,
            r#"export class GameEngine {
  score = 10;
  actors = [{ x: 1, y: 2 }];
  reset() {
    this.score = 0;
    this.actors = [{ x: 1, y: 2 }];
  }
}
"#,
        )
        .unwrap();
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
    fn probe_observations_only_corrobate_mapped_weak_keys() {
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
        assert!(!report.passed, "{report:?}");
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
            Some("absent")
        );
        assert!(
            report
                .missing_evidence
                .contains(&"failure_or_collision_evidence".to_string()),
            "{report:?}"
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
    fn startless_input_state_change_satisfies_generic_interactive_mapping() {
        let dir = tempfile::tempdir().unwrap();
        write_page(
            dir.path(),
            r#""use client";
import { useState } from "react";
export default function Page() {
  const [draft, setDraft] = useState("");
  const [items, setItems] = useState<string[]>([]);
  return (
    <main>
      <input aria-label="Todo" value={draft} onChange={(event) => setDraft(event.target.value)} />
      <button onClick={() => setItems([...items, draft])}>Add</button>
      <ul>{items.map((item) => <li key={item}>{item}</li>)}</ul>
    </main>
  );
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
                "surface_visible": true,
                "start_control_found": false,
                "steps": [
                    "surface_visible",
                    "control_input_dispatched",
                    "input_state_change"
                ],
                "input_before_marker": "items:0,draft:",
                "input_after_marker": "items:0,draft:buy milk"
            }),
        );
        let required = [
            "interactive_ui_source_evidence",
            "visible_interactive_surface_evidence",
            "non_static_screen_evidence",
            "user_input_handler_evidence",
            "stateful_update_evidence",
        ];
        let mut report = report_for(dir.path(), &required);
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

        assert!(report.passed, "{report:?}");
        assert_eq!(
            arbitration
                .records
                .get("visible_interactive_surface_evidence")
                .map(|record| record.behavioral_observation.as_str()),
            Some("surface_visible")
        );
        assert!(
            !report
                .missing_evidence
                .contains(&"visible_interactive_surface_evidence".to_string())
        );
    }

    #[test]
    fn todo_shaped_contract_restart_hook_passes_same_probe_steps() {
        let dir = tempfile::tempdir().unwrap();
        write_page(
            dir.path(),
            r#""use client";
import { useState } from "react";
export default function Page() {
  const [draft, setDraft] = useState("");
  const [items, setItems] = useState<string[]>([]);
  const state = JSON.stringify({ draft, items });
  return (
    <main data-anvil-state={state}>
      <input aria-label="Todo" value={draft} onChange={(event) => setDraft(event.target.value)} />
      <button data-anvil-action="primary" onClick={() => setItems([...items, draft])}>Add</button>
      <button data-anvil-action="restart" onClick={() => { setDraft(""); setItems([]); }}>Clear</button>
      <ul>{items.map((item) => <li key={item}>{item}</li>)}</ul>
    </main>
  );
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
                "probe_mode": "contract",
                "contract_hook_status": "usable",
                "contract_hooks": {
                    "usable": true,
                    "primary_present": true,
                    "restart_present": true,
                    "valid_state_count": 1
                },
                "steps": [
                    "surface_visible",
                    "start_transition",
                    "control_input_dispatched",
                    "input_state_evaluated_after_start",
                    "input_state_change",
                    "recovery_transition"
                ],
                "before_marker": "{\"states\":[{\"index\":0,\"state\":{\"draft\":\"\",\"items\":[]}}]}",
                "after_marker": "{\"states\":[{\"index\":0,\"state\":{\"draft\":\"\",\"items\":[\"anvil probe input\"]}}]}",
                "input_before_marker": "{\"states\":[{\"index\":0,\"state\":{\"draft\":\"\",\"items\":[\"anvil probe input\"]}}]}",
                "input_after_marker": "{\"states\":[{\"index\":0,\"state\":{\"draft\":\"x\",\"items\":[\"anvil probe input\"]}}]}",
                "recovery_before_marker": "{\"states\":[{\"index\":0,\"state\":{\"draft\":\"x\",\"items\":[\"anvil probe input\"]}}]}",
                "recovery_after_marker": "{\"states\":[{\"index\":0,\"state\":{\"draft\":\"\",\"items\":[]}}]}",
                "recovery_transition": true,
                "recovery_transition_status": "observed",
                "state_dimensions_changed": ["draft"]
            }),
        );
        let required = [
            "visible_interactive_surface_evidence",
            "user_input_handler_evidence",
            "stateful_update_evidence",
            "restart_or_recoverable_state_evidence",
        ];
        let mut report = report_for(dir.path(), &required);
        arbitrate(dir.path(), &mut report, &required);

        assert!(report.passed, "{report:?}");
        assert_eq!(
            report
                .evidence_tiers
                .get("restart_or_recoverable_state_evidence")
                .map(String::as_str),
            Some("strong")
        );
    }

    #[test]
    fn probe_ok_does_not_pass_unobserved_weak_failure_key() {
        let dir = tempfile::tempdir().unwrap();
        write_page(
            dir.path(),
            r#""use client";
import { useState } from "react";
export default function Page() {
  const [score, setScore] = useState(0);
  return <main><button onClick={() => setScore(score + 1)}>Start</button><canvas />collision pending score {score}</main>;
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
                    "input_state_evaluated_after_start",
                    "input_state_change"
                ],
                "before_marker": "menu",
                "after_marker": "running",
                "input_before_marker": "score:0",
                "input_after_marker": "score:1"
            }),
        );
        let required = ["failure_or_collision_evidence"];
        let mut report = report_for(dir.path(), &required);
        assert_eq!(
            report
                .evidence_tiers
                .get("failure_or_collision_evidence")
                .map(String::as_str),
            Some("weak"),
            "{report:?}"
        );
        arbitrate(dir.path(), &mut report, &required);

        assert!(!report.passed, "{report:?}");
        assert!(
            report
                .missing_evidence
                .contains(&"failure_or_collision_evidence".to_string()),
            "{report:?}"
        );
        assert_eq!(
            report
                .evidence_tiers
                .get("failure_or_collision_evidence")
                .map(String::as_str),
            Some("absent")
        );
    }

    #[test]
    fn probe_unavailable_marks_weak_behavior_key_unverified() {
        let dir = tempfile::tempdir().unwrap();
        write_cross_file_weak_restart_fixture(dir.path());
        let required = ["restart_or_recoverable_state_evidence"];
        let mut report = report_for(dir.path(), &required);
        assert_eq!(
            report
                .evidence_tiers
                .get("restart_or_recoverable_state_evidence")
                .map(String::as_str),
            Some("weak"),
            "{report:?}"
        );
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
        assert!(report.passed, "{report:?}");
        assert!(
            !report
                .missing_evidence
                .contains(&"restart_or_recoverable_state_evidence".to_string())
        );
        assert!(report.unverified_evidence.contains(
            &"restart_or_recoverable_state_evidence:unverified:probe_unavailable".to_string()
        ));
        assert_eq!(
            report
                .evidence_tiers
                .get("restart_or_recoverable_state_evidence")
                .map(String::as_str),
            Some("unverified:probe_unavailable")
        );
        assert_eq!(arbitration.summary, "partial (probe unavailable)");
        assert_eq!(
            arbitration
                .records
                .get("restart_or_recoverable_state_evidence")
                .map(|record| record.decided_by.as_str()),
            Some("probe_required")
        );
    }

    #[test]
    fn probe_unavailable_static_absent_restart_still_fails() {
        let dir = tempfile::tempdir().unwrap();
        write_page(
            dir.path(),
            r#""use client";
export default function Page() {
  return <main><button>Begin</button><canvas />score enemy collision</main>;
}
"#,
        );
        let required = ["restart_or_recoverable_state_evidence"];
        let mut report = report_for(dir.path(), &required);
        assert_eq!(
            report
                .evidence_tiers
                .get("restart_or_recoverable_state_evidence")
                .map(String::as_str),
            Some("absent"),
            "{report:?}"
        );
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
        assert!(!report.passed, "{report:?}");
        assert!(
            report
                .missing_evidence
                .contains(&"restart_or_recoverable_state_evidence".to_string())
        );
        assert!(report.unverified_evidence.is_empty());
        assert_eq!(arbitration.summary, "partial (probe unavailable)");
    }

    #[test]
    fn probe_available_decides_cross_file_weak_restart_behaviorally() {
        let dir = tempfile::tempdir().unwrap();
        write_cross_file_weak_restart_fixture(dir.path());
        write_interaction(
            dir.path(),
            json!({
                "ok": true,
                "status": "passed",
                "interaction_success": true,
                "interaction_performed": true,
                "input_event_observed": true,
                "state_changed": true,
                "steps": ["surface_visible", "start_transition", "recovery_transition"],
                "before_marker": "gameOver",
                "after_marker": "playing",
                "recovery_transition": true
            }),
        );
        let required = ["restart_or_recoverable_state_evidence"];
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
        assert!(report.unverified_evidence.is_empty());
    }

    #[test]
    fn recovery_transition_not_observed_after_start_and_input_is_repairable_missing_evidence() {
        let dir = tempfile::tempdir().unwrap();
        write_cross_file_weak_restart_fixture(dir.path());
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
                    "recovery_transition:not_observed"
                ],
                "before_marker": "gameOver",
                "after_marker": "playing",
                "input_before_marker": "playing:x=1",
                "input_after_marker": "playing:x=2",
                "recovery_transition": false,
                "recovery_transition_status": "not_observed"
            }),
        );
        let required = ["restart_or_recoverable_state_evidence"];
        let mut report = report_for(dir.path(), &required);
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

        assert!(!report.passed, "{report:?}");
        assert!(report.unverified_evidence.is_empty(), "{report:?}");
        assert!(
            report
                .missing_evidence
                .contains(&"restart_or_recoverable_state_evidence".to_string()),
            "{report:?}"
        );
        assert_eq!(
            report
                .evidence_tiers
                .get("restart_or_recoverable_state_evidence")
                .map(String::as_str),
            Some("absent")
        );
        assert_eq!(
            arbitration
                .records
                .get("restart_or_recoverable_state_evidence")
                .map(|record| record.behavioral_observation.as_str()),
            Some("not_observed_by_probe")
        );
        assert!(!arbitration.summary.contains("probe unavailable"));
    }

    #[test]
    fn infrastructure_failure_marks_weak_behavior_key_unverified() {
        let dir = tempfile::tempdir().unwrap();
        write_cross_file_weak_restart_fixture(dir.path());
        write_interaction(
            dir.path(),
            json!({
                "ok": false,
                "status": "failed",
                "interaction_success": false,
                "stage": "resolving",
                "steps": [],
                "failure_kind": "probe_dependency_missing:playwright_module_missing",
                "remediation": crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION
            }),
        );
        let required = ["restart_or_recoverable_state_evidence"];
        let mut report = report_for(dir.path(), &required);
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

        assert!(report.passed, "{report:?}");
        assert!(report.unverified_evidence.contains(
            &"restart_or_recoverable_state_evidence:unverified:probe_dependency_missing:playwright_module_missing"
                .to_string()
        ));
        assert_eq!(
            report
                .evidence_tiers
                .get("restart_or_recoverable_state_evidence")
                .map(String::as_str),
            Some("unverified:probe_dependency_missing:playwright_module_missing")
        );
        assert_eq!(
            arbitration.summary,
            "partial (probe infrastructure failure: probe_dependency_missing:playwright_module_missing)"
        );
    }

    #[test]
    fn persistence_reload_preserved_behaviorally_satisfies_persistence_evidence() {
        let dir = tempfile::tempdir().unwrap();
        write_page(dir.path(), todo_persistence_page());
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
                    "input_state_evaluated_after_start",
                    "input_state_change",
                    "persistence_reload"
                ],
                "before_marker": "{\"states\":[{\"index\":0,\"state\":{\"items\":[]}}]}",
                "after_marker": "{\"states\":[{\"index\":0,\"state\":{\"items\":[\"anvil probe input\"]}}]}",
                "input_before_marker": "{\"states\":[{\"index\":0,\"state\":{\"items\":[]}}]}",
                "input_after_marker": "{\"states\":[{\"index\":0,\"state\":{\"items\":[\"anvil probe input\"]}}]}",
                "persistence_before_reload_marker": "{\"states\":[{\"index\":0,\"state\":{\"items\":[\"anvil probe input\"]}}]}",
                "persistence_after_reload_marker": "{\"states\":[{\"index\":0,\"state\":{\"items\":[\"anvil probe input\"]}}]}",
                "persistence_after_reload": "preserved",
                "persistence_changed_dimensions": ["items"],
                "state_dimensions_changed": ["items"],
                "action_hooks": ["primary"]
            }),
        );
        let required = ["persistence_evidence"];
        let mut report = report_for(dir.path(), &required);
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

        assert!(report.passed, "{report:?}");
        assert_eq!(
            report
                .evidence_tiers
                .get("persistence_evidence")
                .map(String::as_str),
            Some("strong")
        );
        assert_eq!(
            arbitration
                .records
                .get("persistence_evidence")
                .map(|record| record.behavioral_observation.as_str()),
            Some("preserved_after_reload")
        );
    }

    #[test]
    fn notes_text_entry_echo_behaviorally_satisfies_live_preview_evidence() {
        let dir = tempfile::tempdir().unwrap();
        write_page(dir.path(), notes_live_preview_page());
        write_interaction(
            dir.path(),
            json!({
                "ok": true,
                "status": "passed",
                "interaction_success": true,
                "interaction_performed": true,
                "input_event_observed": true,
                "state_changed": true,
                "text_entry": "entered",
                "text_entry_target": "textarea:data-anvil-action=input",
                "typed_token": "anvil-note",
                "token_echoed": true,
                "text_input_state_change": true,
                "steps": [
                    "surface_visible",
                    "text_entry",
                    "text_input_state_change",
                    "token_echoed"
                ],
                "input_before_marker": "{\"states\":[{\"index\":0,\"state\":{\"draft\":\"\"}}]}",
                "input_after_marker": "{\"states\":[{\"index\":0,\"state\":{\"draft\":\"anvil-note\"}}]}",
                "state_dimensions_changed": ["draft"],
                "action_hooks": ["input"]
            }),
        );
        let required = [
            "user_input_handler_evidence",
            "stateful_update_evidence",
            "live_preview_evidence",
        ];
        let mut report = report_for(dir.path(), &required);
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

        assert!(report.passed, "{report:?}");
        assert_eq!(
            report
                .evidence_tiers
                .get("live_preview_evidence")
                .map(String::as_str),
            Some("strong")
        );
        assert_eq!(
            arbitration
                .records
                .get("live_preview_evidence")
                .map(|record| record.behavioral_observation.as_str()),
            Some("token_echoed")
        );
    }

    #[test]
    fn todo_text_entry_primary_add_echo_satisfies_input_and_preview_evidence() {
        let dir = tempfile::tempdir().unwrap();
        write_page(dir.path(), todo_text_entry_page());
        write_interaction(
            dir.path(),
            json!({
                "ok": true,
                "status": "passed",
                "interaction_success": true,
                "interaction_performed": true,
                "input_event_observed": true,
                "state_changed": true,
                "text_entry": "entered",
                "text_entry_target": "input:data-anvil-action=input",
                "typed_token": "anvil-todo",
                "token_echoed": true,
                "text_input_state_change": true,
                "steps": [
                    "surface_visible",
                    "text_entry",
                    "text_input_state_change",
                    "token_echoed"
                ],
                "input_before_marker": "{\"states\":[{\"index\":0,\"state\":{\"draft\":\"\",\"items\":[]}}]}",
                "input_after_marker": "{\"states\":[{\"index\":0,\"state\":{\"draft\":\"anvil-todo\",\"items\":[\"anvil-todo\"]}}]}",
                "state_dimensions_changed": ["draft", "items"],
                "action_hooks": ["input", "primary"]
            }),
        );
        let required = [
            "user_input_handler_evidence",
            "stateful_update_evidence",
            "live_preview_evidence",
        ];
        let mut report = report_for(dir.path(), &required);
        arbitrate(dir.path(), &mut report, &required);

        assert!(report.passed, "{report:?}");
        assert_eq!(
            report
                .evidence_tiers
                .get("user_input_handler_evidence")
                .map(String::as_str),
            Some("strong")
        );
        assert_eq!(
            report
                .evidence_tiers
                .get("live_preview_evidence")
                .map(String::as_str),
            Some("strong")
        );
    }

    #[test]
    fn token_echo_missing_fails_live_preview_evidence() {
        let dir = tempfile::tempdir().unwrap();
        write_page(dir.path(), notes_live_preview_page());
        write_interaction(
            dir.path(),
            json!({
                "ok": false,
                "status": "failed",
                "failure_kind": "token_echo_missing",
                "interaction_success": false,
                "interaction_performed": true,
                "input_event_observed": true,
                "state_changed": true,
                "text_entry": "entered",
                "text_entry_target": "textarea:data-anvil-action=input",
                "typed_token": "anvil-note",
                "token_echoed": false,
                "text_input_state_change": true,
                "steps": [
                    "surface_visible",
                    "text_entry",
                    "text_input_state_change",
                    "token_echo_missing"
                ],
                "state_dimensions_changed": ["draft"],
                "action_hooks": ["input"]
            }),
        );
        let required = ["live_preview_evidence"];
        let mut report = report_for(dir.path(), &required);
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

        assert!(!report.passed, "{report:?}");
        assert!(
            report
                .missing_evidence
                .contains(&"live_preview_evidence".to_string()),
            "{report:?}"
        );
        assert_eq!(
            arbitration
                .records
                .get("live_preview_evidence")
                .map(|record| record.behavioral_observation.as_str()),
            Some("token_echo_missing")
        );
    }

    #[test]
    fn persistence_reload_prefers_typed_token_survival_signal() {
        let dir = tempfile::tempdir().unwrap();
        write_page(dir.path(), todo_persistence_page());
        write_interaction(
            dir.path(),
            json!({
                "ok": true,
                "status": "passed",
                "interaction_success": true,
                "interaction_performed": true,
                "input_event_observed": true,
                "state_changed": true,
                "text_entry": "entered",
                "text_entry_target": "input:data-anvil-action=input",
                "typed_token": "anvil-persist",
                "token_echoed": true,
                "text_input_state_change": true,
                "steps": [
                    "surface_visible",
                    "text_entry",
                    "text_input_state_change",
                    "token_echoed",
                    "persistence_reload"
                ],
                "persistence_after_reload": "preserved",
                "persistence_changed_dimensions": ["typed_token"],
                "persistence_before_reload_marker": "{\"states\":[{\"index\":0,\"state\":{\"items\":[\"anvil-persist\"]}}]}",
                "persistence_after_reload_marker": "{\"states\":[{\"index\":0,\"state\":{\"items\":[\"anvil-persist\"]}}]}",
                "action_hooks": ["input", "primary"]
            }),
        );
        let required = ["persistence_evidence"];
        let mut report = report_for(dir.path(), &required);
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

        assert!(report.passed, "{report:?}");
        assert_eq!(
            arbitration
                .records
                .get("persistence_evidence")
                .map(|record| record.behavioral_observation.as_str()),
            Some("preserved_after_reload")
        );
    }

    #[test]
    fn persistence_reload_reset_fails_persistence_evidence() {
        let dir = tempfile::tempdir().unwrap();
        write_page(dir.path(), todo_persistence_page());
        write_interaction(
            dir.path(),
            json!({
                "ok": false,
                "status": "failed",
                "failure_kind": "persistence_after_reload_reset",
                "interaction_success": false,
                "interaction_performed": true,
                "input_event_observed": true,
                "state_changed": true,
                "steps": [
                    "surface_visible",
                    "start_transition",
                    "control_input_dispatched",
                    "input_state_evaluated_after_start",
                    "input_state_change",
                    "persistence_reload"
                ],
                "input_before_marker": "{\"states\":[{\"index\":0,\"state\":{\"items\":[]}}]}",
                "input_after_marker": "{\"states\":[{\"index\":0,\"state\":{\"items\":[\"anvil probe input\"]}}]}",
                "persistence_before_reload_marker": "{\"states\":[{\"index\":0,\"state\":{\"items\":[\"anvil probe input\"]}}]}",
                "persistence_after_reload_marker": "{\"states\":[{\"index\":0,\"state\":{\"items\":[]}}]}",
                "persistence_after_reload": "reset",
                "persistence_changed_dimensions": ["items"],
                "state_dimensions_changed": ["items"],
                "action_hooks": ["primary"]
            }),
        );
        let required = ["persistence_evidence"];
        let mut report = report_for(dir.path(), &required);
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

        assert!(!report.passed, "{report:?}");
        assert!(
            report
                .missing_evidence
                .contains(&"persistence_evidence".to_string())
        );
        assert_eq!(
            report
                .evidence_tiers
                .get("persistence_evidence")
                .map(String::as_str),
            Some("absent")
        );
        assert_eq!(
            arbitration
                .records
                .get("persistence_evidence")
                .map(|record| record.behavioral_observation.as_str()),
            Some("reset_after_reload")
        );
    }

    #[test]
    fn persistence_reload_not_evaluated_is_unverified_non_fatal() {
        let dir = tempfile::tempdir().unwrap();
        write_page(dir.path(), todo_persistence_page());
        write_interaction(
            dir.path(),
            json!({
                "ok": true,
                "status": "passed",
                "interaction_success": true,
                "interaction_performed": true,
                "input_event_observed": true,
                "state_changed": false,
                "steps": ["surface_visible", "start_transition", "persistence_reload:not_evaluated"],
                "input_before_marker": "{\"states\":[{\"index\":0,\"state\":{\"items\":[]}}]}",
                "input_after_marker": "{\"states\":[{\"index\":0,\"state\":{\"items\":[]}}]}",
                "persistence_after_reload": "not_evaluated",
                "persistence_changed_dimensions": [],
                "action_hooks": ["primary"]
            }),
        );
        let required = ["persistence_evidence"];
        let mut report = report_for(dir.path(), &required);
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

        assert!(report.passed, "{report:?}");
        assert!(report.missing_evidence.is_empty(), "{report:?}");
        assert!(
            report
                .unverified_evidence
                .contains(&"persistence_evidence:unverified:not_evaluated".to_string())
        );
        assert_eq!(
            report
                .evidence_tiers
                .get("persistence_evidence")
                .map(String::as_str),
            Some("unverified:not_evaluated")
        );
        assert_eq!(
            arbitration
                .records
                .get("persistence_evidence")
                .map(|record| record.behavioral_observation.as_str()),
            Some("not_evaluated")
        );
    }

    #[test]
    fn probe_unavailable_static_persistence_remains_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        write_page(dir.path(), todo_persistence_page());
        let required = ["persistence_evidence"];
        let mut report = report_for(dir.path(), &required);
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

        assert!(report.passed, "{report:?}");
        assert!(report.unverified_evidence.is_empty(), "{report:?}");
        assert_eq!(arbitration.summary, "partial (probe unavailable)");
        assert_eq!(
            report
                .evidence_tiers
                .get("persistence_evidence")
                .map(String::as_str),
            Some("strong")
        );
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
                "remediation": crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION
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
            "partial (probe infrastructure failure: probe_dependency_missing:playwright_module_missing)"
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
