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
    if !step.verify.is_empty()
        || !references_template_owned_artifacts(step)
        || !profile_owns_declared_paths(root, profile, step)
    {
        return (step.clone(), false);
    }
    let Some(checks) = profile_setup_checks(root, profile, goal, step) else {
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
        _ => references_template_owned_artifacts(step) && template_owned_step_scope(step),
    }
}

pub(crate) fn references_template_owned_artifacts(step: &PlanStep) -> bool {
    referenced_template_artifact(step).is_some()
}

pub(crate) fn prompt_mentions_setup(prompt: &str) -> bool {
    let id = prompt_field(prompt, "Current step id:\n").unwrap_or_default();
    let instruction = prompt_field(prompt, "Current step instruction:\n").unwrap_or(prompt);
    step_mentions_setup(id, instruction)
}

pub(crate) fn prompt_references_template_owned_artifacts(prompt: &str) -> bool {
    let Some(step_id) = prompt_field(prompt, "Current step id:\n") else {
        return false;
    };
    let Some(instruction) = prompt_field(prompt, "Current step instruction:\n") else {
        return false;
    };
    let step = PlanStep {
        id: step_id.to_string(),
        kind: prompt_field(prompt, "Current step kind:\n")
            .unwrap_or_default()
            .to_string(),
        expected_result: String::new(),
        instruction: instruction.to_string(),
        expected_paths: list_from_prompt(prompt, "Expected paths after this step:\n"),
        verify: verification_commands_from_prompt(prompt),
    };
    references_template_owned_artifacts(&step) && template_owned_step_scope(&step)
}

pub(crate) fn verification_commands_from_prompt(prompt: &str) -> Vec<String> {
    list_from_prompt(prompt, "Verification commands for this step:\n")
}

fn list_from_prompt(prompt: &str, marker: &str) -> Vec<String> {
    prompt_field(prompt, marker)
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
        if !references_template_owned_artifacts(step) {
            continue;
        }
        let Some(checks) = profile_setup_checks(root, profile, goal, step) else {
            continue;
        };
        if !profile_owns_declared_paths(root, profile, step) {
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
    step: &PlanStep,
) -> Option<SetupStepChecks> {
    if !is_nextjs_profile(profile) {
        return None;
    }
    let artifact_knowledge =
        &crate::planner::profiles::nextjs::knowledge::get().template_owned_artifacts;
    let marker = match referenced_template_artifact(step)? {
        TemplateOwnedArtifact::PackageManifest => &artifact_knowledge.package_check_marker,
        TemplateOwnedArtifact::ScaffoldConfiguration => &artifact_knowledge.scaffold_check_marker,
    };
    crate::planner::profiles::nextjs::setup_step_checks(root, goal, marker, "")
}

fn profile_owns_declared_paths(root: &Path, profile: &str, step: &PlanStep) -> bool {
    if !is_nextjs_profile(profile) {
        return false;
    }
    let owned = crate::planner::profiles::nextjs::setup_scaffold_paths(root);
    step.expected_paths.iter().all(|path| owned.contains(path))
        && (step.step_kind() == StepKind::Setup
            || step
                .expected_paths
                .iter()
                .all(|path| template_owned_artifact_path(path)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateOwnedArtifact {
    PackageManifest,
    ScaffoldConfiguration,
}

fn referenced_template_artifact(step: &PlanStep) -> Option<TemplateOwnedArtifact> {
    let mut scaffold_referenced = false;
    for path in &step.expected_paths {
        if package_manifest_path(path) {
            return Some(TemplateOwnedArtifact::PackageManifest);
        }
        scaffold_referenced |= template_owned_artifact_path(path);
    }
    let authored_instruction = step
        .instruction
        .split_once("\n\nProfile contract:")
        .map(|(instruction, _)| instruction)
        .unwrap_or(&step.instruction);
    for text in std::iter::once(step.id.as_str())
        .chain(std::iter::once(authored_instruction))
        .chain(step.verify.iter().map(String::as_str))
    {
        match template_owned_text_reference(text) {
            Some(TemplateOwnedArtifact::PackageManifest) => {
                return Some(TemplateOwnedArtifact::PackageManifest);
            }
            Some(TemplateOwnedArtifact::ScaffoldConfiguration) => {
                scaffold_referenced = true;
            }
            None => {}
        }
    }
    scaffold_referenced.then_some(TemplateOwnedArtifact::ScaffoldConfiguration)
}

fn template_owned_text_reference(text: &str) -> Option<TemplateOwnedArtifact> {
    let lower = text.replace('\\', "/").to_ascii_lowercase();
    let tokens = lower
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let knowledge = &crate::planner::profiles::nextjs::knowledge::get().template_owned_artifacts;
    let package_reference = knowledge
        .package_phrases
        .iter()
        .any(|phrase| lower.contains(phrase))
        || tokens
            .iter()
            .any(|token| knowledge.package_tokens.iter().any(|item| item == *token));
    if package_reference {
        return Some(TemplateOwnedArtifact::PackageManifest);
    }
    let scaffold_reference = knowledge
        .scaffold_phrases
        .iter()
        .any(|phrase| lower.contains(phrase))
        || tokens
            .iter()
            .any(|token| knowledge.scaffold_tokens.iter().any(|item| item == *token));
    scaffold_reference.then_some(TemplateOwnedArtifact::ScaffoldConfiguration)
}

fn package_manifest_path(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_ascii_lowercase();
    lower.rsplit('/').next().is_some_and(|name| {
        crate::planner::profiles::nextjs::knowledge::get()
            .template_owned_artifacts
            .package_manifest_names
            .iter()
            .any(|item| item == name)
    })
}

fn template_owned_artifact_path(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_ascii_lowercase();
    let knowledge = &crate::planner::profiles::nextjs::knowledge::get().template_owned_artifacts;
    package_manifest_path(&lower)
        || knowledge
            .artifact_path_suffixes
            .iter()
            .any(|suffix| lower.ends_with(suffix))
        || knowledge
            .artifact_path_contains
            .iter()
            .any(|fragment| lower.contains(fragment))
}

fn template_owned_step_scope(step: &PlanStep) -> bool {
    step.step_kind() == StepKind::Setup
        || step
            .expected_paths
            .iter()
            .all(|path| template_owned_artifact_path(path))
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

    fn ensure_port_scripts_implement_step() -> PlanStep {
        PlanStep {
            id: "ensure-port-scripts".to_string(),
            kind: "implement".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Update package.json scripts to use port 3011.".to_string(),
            expected_paths: Vec::new(),
            verify: Vec::new(),
        }
    }

    #[test]
    fn template_owned_artifact_predicate_uses_every_step_field_conservatively() {
        let mut step = PlanStep {
            id: "implement-gameplay".to_string(),
            kind: "implement".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Implement Breakout paddle movement and collision logic.".to_string(),
            expected_paths: vec!["src/app/page.tsx".to_string()],
            verify: vec!["npm run build".to_string()],
        };
        assert!(!references_template_owned_artifacts(&step));

        step.id = "ensure-port-scripts".to_string();
        assert!(references_template_owned_artifacts(&step));

        step.id = "implement-gameplay".to_string();
        step.instruction = "Check the existing tsconfig.json before continuing.".to_string();
        assert!(references_template_owned_artifacts(&step));

        step.instruction = "Configure the project before continuing.".to_string();
        step.expected_paths = vec!["package.json".to_string()];
        assert!(references_template_owned_artifacts(&step));

        step.expected_paths.clear();
        step.verify = vec!["node -p \"require('./package.json').scripts.dev\"".to_string()];
        assert!(references_template_owned_artifacts(&step));

        step.verify = vec!["npm run build".to_string()];
        assert!(!references_template_owned_artifacts(&step));
    }

    #[test]
    fn template_owned_prompt_feedback_rejects_mixed_final_acceptance_scope() {
        let ensure_port = "Current step id:\nensure-port-scripts\n\nCurrent step kind:\nimplement\n\nCurrent step instruction:\nConfirm package.json scripts use port 3011.\n\nExpected paths after this step:\n- package.json\n\nVerification commands for this step:\n- node -p \"require('./package.json').scripts.dev\"";
        assert!(prompt_references_template_owned_artifacts(ensure_port));

        let mixed_repair = "Current step id:\nfinal-acceptance-repair\n\nCurrent step kind:\nimplement\n\nCurrent step instruction:\nRepair the compile failure.\n\nExpected paths after this step:\n- package.json\n- src/app/page.tsx\n\nVerification commands for this step:\n- npm run build";
        assert!(!prompt_references_template_owned_artifacts(mixed_repair));
        assert!(!prompt_references_template_owned_artifacts(
            "Repair final acceptance for package.json and src/app/page.tsx."
        ));
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
    fn empty_verify_implement_port_step_synthesizes_passing_profile_precheck() {
        let dir = tempfile::tempdir().unwrap();
        write_package(dir.path(), "next dev -p 3011");

        let (runtime, synthesized) = runtime_step_with_profile_checks(
            dir.path(),
            "nextjs",
            "Build a Next.js app on port 3011",
            &ensure_port_scripts_implement_step(),
        );
        let (report, _) = verify_step_with_profile_setup_observed_with_offline(
            dir.path(),
            &runtime,
            Some("nextjs"),
            NodeDependencySetupAuthority::None,
            true,
        );

        assert!(synthesized);
        assert_eq!(runtime.kind, "implement");
        assert_eq!(runtime.expected_paths, ["package.json"]);
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
    fn preset_implementation_port_step_is_converted_independent_of_kind() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut plan = StepPlan {
            goal: "Implement the app".to_string(),
            steps: vec![ensure_port_scripts_implement_step()],
        };

        let count = convert_preset_phase_setup_steps(
            &mut plan,
            dir.path(),
            "nextjs",
            "Build a Next.js app on port 3011",
            Some("core-implementation"),
            true,
            Some(&events),
        );

        assert_eq!(count, 1);
        assert_eq!(plan.steps[0].kind, "verify");
        assert_eq!(plan.steps[0].expected_paths, ["package.json"]);
        assert!(!plan.steps[0].verify.is_empty());
        let event = std::fs::read_to_string(events).unwrap();
        assert!(event.contains("\"event\":\"preset_step_converted\""));
        assert!(event.contains("\"step_id\":\"ensure-port-scripts\""));
    }

    #[test]
    fn preset_game_logic_implement_step_is_not_converted() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = StepPlan {
            goal: "Implement Breakout".to_string(),
            steps: vec![PlanStep {
                id: "implement-gameplay".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Implement paddle movement, ball collision, and scoring.\n\nProfile contract:\nKeep package scripts on port 3011.".to_string(),
                expected_paths: vec!["src/app/page.tsx".to_string()],
                verify: vec!["npm run build".to_string()],
            }],
        };

        assert!(!references_template_owned_artifacts(&plan.steps[0]));
        assert_eq!(
            convert_preset_phase_setup_steps(
                &mut plan,
                dir.path(),
                "nextjs",
                "Build a Next.js app on port 3011",
                Some("core-implementation"),
                true,
                None,
            ),
            0
        );
        assert_eq!(plan.steps[0].kind, "implement");
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
