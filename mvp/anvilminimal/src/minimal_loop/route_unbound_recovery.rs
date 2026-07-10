use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::json;

use crate::eval_events;
use crate::minimal_loop::evidence::RuntimeAcceptanceReport;
use crate::minimal_loop::stagnation_escalation::{
    WriteRequiredSelectionReason, WriteRequiredState, WriteRequiredTargetSelection,
};

pub(crate) const ROUTE_UNBOUND_WRITE_REQUIRED_THRESHOLD: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteUnboundRecoveryStage {
    DeterministicGuidance,
    Observed,
    WriteRequired,
}

impl RouteUnboundRecoveryStage {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::DeterministicGuidance => "deterministic_guidance",
            Self::Observed => "observed",
            Self::WriteRequired => "write_required",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouteUnboundRecoveryDecision {
    pub(crate) stage: RouteUnboundRecoveryStage,
    pub(crate) route_bound_path: String,
    pub(crate) unbound_path: String,
    pub(crate) missing_evidence: Vec<String>,
    pub(crate) failure_count: usize,
    pub(crate) feedback: String,
}

impl RouteUnboundRecoveryDecision {
    pub(crate) fn write_required_selection(&self) -> WriteRequiredTargetSelection {
        WriteRequiredTargetSelection {
            selected_targets: vec![self.route_bound_path.clone()],
            selection_reason: WriteRequiredSelectionReason::EvidenceMapped,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct RouteUnboundRecoveryState {
    counts: BTreeMap<String, usize>,
    guidance_emitted: BTreeSet<String>,
    write_required_emitted: BTreeSet<String>,
}

impl RouteUnboundRecoveryState {
    pub(crate) fn observe(
        &mut self,
        root: &Path,
        eval_events_path: Option<&Path>,
        report: &RuntimeAcceptanceReport,
    ) -> Option<RouteUnboundRecoveryDecision> {
        let case = RouteUnboundCase::from_report(report)?;
        let key = case.key();
        let count = self.counts.entry(key.clone()).or_default();
        *count = count.saturating_add(1);
        let failure_count = *count;
        let stage = if failure_count >= ROUTE_UNBOUND_WRITE_REQUIRED_THRESHOLD
            && self.write_required_emitted.insert(key.clone())
        {
            RouteUnboundRecoveryStage::WriteRequired
        } else if self.guidance_emitted.insert(key.clone()) {
            RouteUnboundRecoveryStage::DeterministicGuidance
        } else {
            RouteUnboundRecoveryStage::Observed
        };
        emit_recovery_event(eval_events_path, stage, &case, failure_count);
        if stage == RouteUnboundRecoveryStage::Observed {
            return None;
        }
        let feedback = match stage {
            RouteUnboundRecoveryStage::DeterministicGuidance => case.guidance(root, failure_count),
            RouteUnboundRecoveryStage::WriteRequired => case.write_required_feedback(failure_count),
            RouteUnboundRecoveryStage::Observed => unreachable!(),
        };
        Some(RouteUnboundRecoveryDecision {
            stage,
            route_bound_path: case.route_bound_path,
            unbound_path: case.unbound_path,
            missing_evidence: case.missing_evidence,
            failure_count,
            feedback,
        })
    }
}

pub(crate) fn feedback_or_route_unbound_recovery(
    state: &mut RouteUnboundRecoveryState,
    write_required_state: &mut WriteRequiredState,
    root: &Path,
    eval_events_path: Option<&Path>,
    report: &RuntimeAcceptanceReport,
    fallback_feedback: String,
) -> String {
    let Some(decision) = state.observe(root, eval_events_path, report) else {
        return fallback_feedback;
    };
    if decision.stage == RouteUnboundRecoveryStage::WriteRequired {
        write_required_state.activate_with_feedback(
            decision.write_required_selection(),
            decision.feedback.clone(),
        );
    }
    decision.feedback
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RouteUnboundCase {
    route_bound_path: String,
    unbound_path: String,
    missing_evidence: Vec<String>,
}

impl RouteUnboundCase {
    fn from_report(report: &RuntimeAcceptanceReport) -> Option<Self> {
        if report.missing_evidence.is_empty() {
            return None;
        }
        let missing = report
            .missing_evidence
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let weak_paths = report
            .weak_evidence
            .iter()
            .filter_map(|item| item.strip_prefix("route_unbound:"))
            .collect::<BTreeSet<_>>();
        if weak_paths.is_empty() {
            return None;
        }
        let mut evidence_by_path: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for diagnostic in &report.diagnostics {
            let Some(rest) = diagnostic.strip_prefix("route_unbound_capability_artifact:") else {
                continue;
            };
            let Some((path, evidence)) = rest.rsplit_once(':') else {
                continue;
            };
            if weak_paths.contains(path) && missing.contains(evidence) {
                evidence_by_path
                    .entry(path.to_string())
                    .or_default()
                    .insert(evidence.to_string());
            }
        }
        let (unbound_path, evidence) = evidence_by_path.into_iter().next()?;
        Some(Self {
            route_bound_path: route_bound_target(report),
            unbound_path,
            missing_evidence: evidence.into_iter().collect(),
        })
    }

    fn key(&self) -> String {
        format!(
            "{}|{}|{}",
            self.route_bound_path,
            self.unbound_path,
            self.missing_evidence.join(",")
        )
    }

    fn guidance(&self, root: &Path, failure_count: usize) -> String {
        format!(
            "Route-bound wiring required after route_unbound evidence #{failure_count}.\n\
Target route-bound file: `{}`\n\
Unbound capability file: `{}`\n\
Missing evidence currently stranded off-route: {}\n\n\
Update `{}` now: import `{}` from the route page, render/connect it from the page, and put the route-bound observability hooks on the page-rendered surface: `data-anvil-action=\"primary\"`, `data-anvil-action=\"restart\"`, and `data-anvil-state` with meaningful JSON state. Do not leave the gameplay only in `{}`.\n\n\
Current `{}` excerpt:\n{}",
            self.route_bound_path,
            self.unbound_path,
            self.missing_evidence.join(", "),
            self.route_bound_path,
            self.unbound_path,
            self.unbound_path,
            self.route_bound_path,
            route_bound_excerpt(root, &self.route_bound_path)
        )
    }

    fn write_required_feedback(&self, failure_count: usize) -> String {
        format!(
            "Route-unbound capability evidence is still unresolved after {failure_count} observations. write_required is now targeting `{}` with selection_reason=evidence_mapped. Use Write/Edit on `{}` now: import and render `{}`, and attach `data-anvil-action=\"primary\"`, `data-anvil-action=\"restart\"`, and `data-anvil-state` on the route-bound rendered surface.",
            self.route_bound_path, self.route_bound_path, self.unbound_path
        )
    }
}

fn emit_recovery_event(
    eval_events_path: Option<&Path>,
    stage: RouteUnboundRecoveryStage,
    case: &RouteUnboundCase,
    failure_count: usize,
) {
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "route_unbound_recovery",
            "stage": stage.as_str(),
            "route_bound_path": case.route_bound_path,
            "unbound_path": case.unbound_path,
            "missing_evidence": case.missing_evidence,
            "failure_count": failure_count,
            "selection_reason": if stage == RouteUnboundRecoveryStage::WriteRequired {
                "evidence_mapped"
            } else {
                ""
            },
        }),
    );
}

fn route_bound_target(report: &RuntimeAcceptanceReport) -> String {
    report
        .artifact_obligations
        .iter()
        .find(|artifact| artifact.route_bound && artifact.path == "src/app/page.tsx")
        .or_else(|| {
            report
                .artifact_obligations
                .iter()
                .find(|artifact| artifact.route_bound)
        })
        .map(|artifact| artifact.path.clone())
        .unwrap_or_else(|| "src/app/page.tsx".to_string())
}

fn route_bound_excerpt(root: &Path, route_bound_path: &str) -> String {
    let path = root.join(route_bound_path);
    let Ok(content) = std::fs::read_to_string(path) else {
        return "(route-bound file is missing)".to_string();
    };
    let lines = content.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        return "   1 | ".to_string();
    }
    let center = lines
        .iter()
        .position(|line| {
            line.contains("data-anvil")
                || line.contains("<main")
                || line.contains("return")
                || line.contains("export default")
        })
        .unwrap_or(0);
    let start = center.saturating_sub(3);
    let end = (start + 12).min(lines.len());
    (start..end)
        .map(|index| format!("{:>4} | {}", index + 1, lines[index]))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minimal_loop::evidence::verify_runtime_acceptance;

    fn write_unbound_game(root: &Path) {
        std::fs::create_dir_all(root.join("src/app")).unwrap();
        std::fs::create_dir_all(root.join("src/components")).unwrap();
        std::fs::write(
            root.join("src/app/page.tsx"),
            r#""use client";
export default function Page(){
  return <main><h1>Arcade</h1><button>Start</button></main>;
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/components/SpaceInvaders.tsx"),
            game_component(),
        )
        .unwrap();
    }

    fn write_bound_game(root: &Path) {
        std::fs::write(
            root.join("src/app/page.tsx"),
            r#""use client";
import SpaceInvaders from "../components/SpaceInvaders";
export default function Page(){
  return <main data-anvil-state={JSON.stringify({ score: 0, phase: "ready" })}>
    <button data-anvil-action="primary">Start</button>
    <button data-anvil-action="restart">Restart</button>
    <SpaceInvaders />
  </main>;
}
"#,
        )
        .unwrap();
    }

    fn game_component() -> &'static str {
        r#""use client";
import { useEffect, useState } from "react";
export default function SpaceInvaders(){
  const [score, setScore] = useState(0);
  const [enemies, setEnemies] = useState([{ x: 10, y: 20 }]);
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") setScore((value) => value + 1);
    };
    const frame = requestAnimationFrame(() => {
      const collision = enemies.some((enemy) => enemy.x > 0);
      if (collision) setEnemies([{ x: 20, y: 20 }]);
    });
    window.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [enemies]);
  return <section><canvas /><p>score {score} enemy collision</p></section>;
}
"#
    }

    fn report(root: &Path) -> RuntimeAcceptanceReport {
        verify_runtime_acceptance(
            root,
            &["src/app/page.tsx".to_string()],
            &[],
            &[
                "player_control".to_string(),
                "progression_or_score".to_string(),
            ],
            &[],
            &["implementation".to_string()],
            &[],
        )
    }

    #[test]
    fn unbound_game_with_hookless_page_gets_deterministic_guidance_once() {
        let dir = tempfile::tempdir().unwrap();
        write_unbound_game(dir.path());
        let report = report(dir.path());
        let mut state = RouteUnboundRecoveryState::default();

        let first = state.observe(dir.path(), None, &report).unwrap();
        let second = state.observe(dir.path(), None, &report);

        assert_eq!(
            first.stage,
            RouteUnboundRecoveryStage::DeterministicGuidance
        );
        assert!(second.is_none());
        assert!(
            first
                .feedback
                .contains("Target route-bound file: `src/app/page.tsx`")
        );
        assert!(
            first
                .feedback
                .contains("Unbound capability file: `src/components/SpaceInvaders.tsx`")
        );
        assert!(
            first
                .feedback
                .contains("import `src/components/SpaceInvaders.tsx`")
        );
        assert!(first.feedback.contains("data-anvil-action=\"primary\""));
        assert!(first.feedback.contains("data-anvil-action=\"restart\""));
        assert!(first.feedback.contains("data-anvil-state"));
        assert!(first.feedback.contains("3 |   return <main>"));
    }

    #[test]
    fn imported_component_and_page_hooks_resolve_route_unbound_case() {
        let dir = tempfile::tempdir().unwrap();
        write_unbound_game(dir.path());
        assert!(RouteUnboundCase::from_report(&report(dir.path())).is_some());

        write_bound_game(dir.path());
        let report = report(dir.path());

        assert!(report.passed, "{report:?}");
        assert!(RouteUnboundCase::from_report(&report).is_none());
    }

    #[test]
    fn repeated_route_unbound_observations_trigger_write_required_decision() {
        let dir = tempfile::tempdir().unwrap();
        write_unbound_game(dir.path());
        let events = dir.path().join(".anvil/runs/route-unbound/events.jsonl");
        let report = report(dir.path());
        let mut state = RouteUnboundRecoveryState::default();
        let mut write_required = WriteRequiredState::default();
        let mut feedback = String::new();

        for _ in 0..ROUTE_UNBOUND_WRITE_REQUIRED_THRESHOLD {
            feedback = feedback_or_route_unbound_recovery(
                &mut state,
                &mut write_required,
                dir.path(),
                Some(&events),
                &report,
                "generic feedback".to_string(),
            );
        }
        let events_text = std::fs::read_to_string(events).unwrap();

        assert_eq!(write_required.selected_targets(), vec!["src/app/page.tsx"]);
        assert_eq!(
            write_required.selection_reason(),
            Some(WriteRequiredSelectionReason::EvidenceMapped)
        );
        assert!(feedback.contains("write_required is now targeting `src/app/page.tsx`"));
        assert!(events_text.contains(r#""event":"route_unbound_recovery""#));
        assert!(events_text.contains(r#""stage":"deterministic_guidance""#));
        assert!(events_text.contains(r#""stage":"write_required""#));
        assert!(events_text.contains(r#""failure_count":5"#));
    }
}
