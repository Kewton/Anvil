use std::path::Path;

use serde_json::json;

use crate::eval_events;
use crate::planner::profile::is_nextjs_profile;
use crate::planner::profiles::nextjs::SetupStepChecks;
use crate::planner::step_plan::{PlanStep, StepKind, StepPlan};

pub(crate) fn runtime_step_with_profile_checks(
    root: &Path,
    profile: &str,
    goal: &str,
    step: &PlanStep,
) -> (PlanStep, bool) {
    if step.step_kind() != StepKind::Setup || !step.verify.is_empty() {
        return (step.clone(), false);
    }
    let Some(checks) = profile_setup_checks(root, profile, goal, &step.id, &step.instruction)
    else {
        return (step.clone(), false);
    };
    let mut runtime_step = step.clone();
    merge_unique_paths(&mut runtime_step.expected_paths, checks.expected_paths);
    runtime_step.verify = checks.verify_commands;
    runtime_step.instruction = format!(
        "{}\n\nBefore changing files, run the declared profile checks. If they already pass, report this step complete; otherwise repair the failing profile-owned setup contract.",
        step.instruction
    );
    (runtime_step, true)
}

pub(crate) fn step_short_circuit_precheck_applicable(step: &PlanStep) -> bool {
    if step.expected_paths.is_empty() && step.verify.is_empty() {
        return false;
    }
    match step.step_kind() {
        StepKind::Setup => step_mentions_setup(&step.id, &step.instruction),
        StepKind::Verify => !step.expected_paths.is_empty(),
        _ => false,
    }
}

pub(crate) fn prompt_mentions_setup(prompt: &str) -> bool {
    let id = prompt_field(prompt, "Current step id:\n").unwrap_or_default();
    let instruction = prompt_field(prompt, "Current step instruction:\n").unwrap_or(prompt);
    step_mentions_setup(id, instruction)
}

pub(crate) fn verification_commands_from_prompt(prompt: &str) -> Vec<String> {
    prompt_field(prompt, "Verification commands for this step:\n")
        .into_iter()
        .flat_map(str::lines)
        .filter_map(|line| line.trim().strip_prefix("- "))
        .filter(|command| *command != "none")
        .map(str::to_string)
        .collect()
}

pub(crate) fn convert_preset_phase_setup_steps(
    plan: &mut StepPlan,
    root: &Path,
    profile: &str,
    goal: &str,
    phase_id: Option<&str>,
    preset_phase: bool,
    eval_events_path: Option<&Path>,
) -> usize {
    if !preset_phase || !phase_id.is_some_and(is_implementation_phase) {
        return 0;
    }
    let mut converted = 0;
    for step in &mut plan.steps {
        if step.step_kind() != StepKind::Setup {
            continue;
        }
        let Some(checks) = profile_setup_checks(root, profile, goal, &step.id, &step.instruction)
        else {
            continue;
        };
        if !profile_owns_declared_paths(root, profile, &step.expected_paths) {
            continue;
        }
        step.kind = "verify".to_string();
        step.expected_result = "pass".to_string();
        step.instruction = format!(
            "Verify the profile-owned {} contract by running every declared check and report any exact failure.",
            checks.ownership
        );
        merge_unique_paths(&mut step.expected_paths, checks.expected_paths);
        step.verify = checks.verify_commands;
        eval_events::emit(
            eval_events_path,
            json!({
                "event": "preset_step_converted",
                "phase_id": phase_id,
                "step_id": step.id,
                "ownership": checks.ownership,
            }),
        );
        converted += 1;
    }
    converted
}

fn profile_setup_checks(
    root: &Path,
    profile: &str,
    goal: &str,
    step_id: &str,
    instruction: &str,
) -> Option<SetupStepChecks> {
    if !is_nextjs_profile(profile) {
        return None;
    }
    crate::planner::profiles::nextjs::setup_step_checks(root, goal, step_id, instruction)
}

fn profile_owns_declared_paths(root: &Path, profile: &str, paths: &[String]) -> bool {
    if !is_nextjs_profile(profile) {
        return false;
    }
    let owned = crate::planner::profiles::nextjs::setup_scaffold_paths(root);
    paths.iter().all(|path| owned.contains(path))
}

fn merge_unique_paths(paths: &mut Vec<String>, additional: Vec<String>) {
    for path in additional {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
}

fn step_mentions_setup(step_id: &str, instruction: &str) -> bool {
    let lower = format!("{step_id} {instruction}").to_ascii_lowercase();
    lower.contains("set up")
        || lower.contains("pre-scaffold")
        || lower.contains("pre-provision")
        || lower.contains("already present")
        || lower
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .any(|token| {
                matches!(
                    token,
                    "setup" | "scaffold" | "script" | "scripts" | "config" | "manifest"
                )
            })
}

fn is_implementation_phase(phase_id: &str) -> bool {
    let lower = phase_id.to_ascii_lowercase();
    ["implement", "core", "wiring", "contract"]
        .iter()
        .any(|token| lower.contains(token))
}

fn prompt_field<'a>(prompt: &'a str, marker: &str) -> Option<&'a str> {
    let after_marker = prompt.split_once(marker)?.1;
    Some(
        after_marker
            .split("\n\n")
            .next()
            .unwrap_or(after_marker)
            .trim(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minimal_loop::dependency_setup::NodeDependencySetupAuthority;
    use crate::planner::verify::verify_step_with_profile_setup_observed_with_offline;

    fn setup_scripts_step() -> PlanStep {
        PlanStep {
            id: "setup-scripts".to_string(),
            kind: "setup".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Confirm the package is ready.".to_string(),
            expected_paths: Vec::new(),
            verify: Vec::new(),
        }
    }

    #[test]
    fn setup_classification_uses_step_id_tokens() {
        for id in ["setup", "package-scripts", "app-config", "project-scaffold"] {
            assert!(
                step_mentions_setup(id, "Confirm the existing state."),
                "{id}"
            );
        }
    }

    fn write_package(root: &Path, dev: &str) {
        std::fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/next")).unwrap();
        std::fs::write(root.join("node_modules/.bin/next"), "").unwrap();
        std::fs::write(
            root.join("package.json"),
            format!(
                r#"{{"scripts":{{"dev":"{dev}","start":"next start -p 3011","build":"next build"}}}}"#
            ),
        )
        .unwrap();
    }

    fn write_scaffold(root: &Path) {
        write_package(root, "next dev -p 3011");
        for path in crate::planner::profiles::nextjs::setup_invariant_required_paths(root) {
            let path = root.join(path);
            if path.ends_with("package.json") {
                continue;
            }
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
    }

    #[test]
    fn empty_verify_setup_scripts_synthesizes_passing_profile_precheck() {
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), "next dev -p 3011");

        let (runtime, synthesized) = runtime_step_with_profile_checks(
            dir.path(),
            "nextjs",
            "Build a Next.js app",
            &setup_scripts_step(),
        );
        let (report, _) = verify_step_with_profile_setup_observed_with_offline(
            dir.path(),
            &runtime,
            Some("nextjs"),
            NodeDependencySetupAuthority::None,
            true,
        );

        assert!(synthesized);
        assert!(step_short_circuit_precheck_applicable(&runtime));
        assert!(report.is_pass(), "{}", report.primary_reason());
    }

    #[test]
    fn setup_scripts_with_missing_port_does_not_pass_profile_precheck() {
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), "next dev");

        let (runtime, synthesized) = runtime_step_with_profile_checks(
            dir.path(),
            "nextjs",
            "Build a Next.js app",
            &setup_scripts_step(),
        );
        let (report, _) = verify_step_with_profile_setup_observed_with_offline(
            dir.path(),
            &runtime,
            Some("nextjs"),
            NodeDependencySetupAuthority::None,
            true,
        );

        assert!(synthesized);
        assert!(!report.is_pass());
        assert!(!report.command_failures.is_empty());
    }

    #[test]
    fn empty_verify_scaffold_config_synthesizes_profile_file_checks() {
        let dir = tempfile::tempdir().unwrap();
        write_scaffold(dir.path());
        let step = PlanStep {
            id: "scaffold-config".to_string(),
            kind: "setup".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Confirm the project shell.".to_string(),
            expected_paths: Vec::new(),
            verify: Vec::new(),
        };

        let (runtime, synthesized) =
            runtime_step_with_profile_checks(dir.path(), "nextjs", "Build a Next.js app", &step);
        let (report, _) = verify_step_with_profile_setup_observed_with_offline(
            dir.path(),
            &runtime,
            Some("nextjs"),
            NodeDependencySetupAuthority::None,
            true,
        );

        assert!(synthesized);
        assert!(
            runtime
                .verify
                .iter()
                .any(|command| command.starts_with("test -f"))
        );
        assert!(report.is_pass(), "{}", report.primary_reason());
    }

    #[test]
    fn preset_implementation_setup_is_converted_and_emitted() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut plan = StepPlan {
            goal: "Implement the app".to_string(),
            steps: vec![setup_scripts_step()],
        };

        let count = convert_preset_phase_setup_steps(
            &mut plan,
            dir.path(),
            "nextjs",
            "Build a Next.js app",
            Some("core-implementation"),
            true,
            Some(&events),
        );

        assert_eq!(count, 1);
        assert_eq!(plan.steps[0].kind, "verify");
        assert!(!plan.steps[0].verify.is_empty());
        let event = std::fs::read_to_string(events).unwrap();
        assert!(event.contains("\"event\":\"preset_step_converted\""));
        assert!(event.contains("\"step_id\":\"setup-scripts\""));
    }

    #[test]
    fn setup_step_with_non_template_path_is_not_converted() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = StepPlan {
            goal: "Implement a game".to_string(),
            steps: vec![PlanStep {
                id: "setup-scripts".to_string(),
                kind: "setup".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Configure package scripts and implement the game.".to_string(),
                expected_paths: vec!["src/app/game.tsx".to_string()],
                verify: Vec::new(),
            }],
        };

        assert_eq!(
            convert_preset_phase_setup_steps(
                &mut plan,
                dir.path(),
                "nextjs",
                "Build a Next.js app",
                Some("core-implementation"),
                true,
                None,
            ),
            0
        );
        assert_eq!(plan.steps[0].kind, "setup");
    }

    #[test]
    fn dependency_setup_without_known_profile_checks_keeps_original_runtime_step() {
        let dir = tempfile::tempdir().unwrap();
        let step = PlanStep {
            id: "install-dependencies".to_string(),
            kind: "setup".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Install dependencies.".to_string(),
            expected_paths: Vec::new(),
            verify: Vec::new(),
        };

        let (runtime, synthesized) =
            runtime_step_with_profile_checks(dir.path(), "nextjs", "Build a Next.js app", &step);

        assert!(!synthesized);
        assert_eq!(runtime, step);
    }
}
