use std::collections::BTreeSet;
use std::path::Path;

use crate::planner::lint::{
    VerifyDependencyOrderViolationKind, diagnose_step_plan_dependency_order,
};
use crate::planner::step_plan::{PlanStep, StepKind, StepPlan};
use crate::planner::verify::{VerifyCommandViolationKind, diagnose_verify_command};
use crate::tools::path_guard::validate_workspace_relative;

const BROWSER_READINESS_NOTE: &str =
    "Browser readiness is verified by the runtime at final acceptance.";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SanitizerReport {
    pub removed_commands: Vec<SanitizedCommandRecord>,
    pub substituted_commands: Vec<SanitizedSubstitutionRecord>,
    pub moved_commands: Vec<SanitizedMoveRecord>,
    pub dropped_commands: Vec<SanitizedCommandRecord>,
    pub retyped_steps: Vec<SanitizedRetypeRecord>,
    pub instruction_notes: Vec<SanitizedInstructionNote>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedCommandRecord {
    pub step_id: String,
    pub command: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedSubstitutionRecord {
    pub step_id: String,
    pub removed_command: String,
    pub substituted_command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedMoveRecord {
    pub from_step_id: String,
    pub to_step_id: String,
    pub command: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedRetypeRecord {
    pub step_id: String,
    pub from_kind: String,
    pub to_kind: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedInstructionNote {
    pub step_id: String,
    pub note: String,
}

impl SanitizerReport {
    pub fn is_empty(&self) -> bool {
        self.removed_commands.is_empty()
            && self.substituted_commands.is_empty()
            && self.moved_commands.is_empty()
            && self.dropped_commands.is_empty()
            && self.retyped_steps.is_empty()
            && self.instruction_notes.is_empty()
    }
}

pub fn sanitize_step_plan_against_policy(
    plan: &mut StepPlan,
    workspace_root: Option<&Path>,
) -> SanitizerReport {
    let mut report = SanitizerReport::default();
    remove_setup_or_dev_server_verify_commands(plan, &mut report);
    let should_retype_manifest_step = !report.removed_commands.is_empty()
        || !diagnose_step_plan_dependency_order(plan, workspace_root).is_empty();
    if should_retype_manifest_step {
        retype_manifest_step_if_needed(plan, &mut report);
    }
    move_dependency_order_commands(plan, workspace_root, &mut report);
    normalize_empty_verify_steps(plan, &mut report);
    dedupe_verify_commands(plan);
    report
}

fn remove_setup_or_dev_server_verify_commands(plan: &mut StepPlan, report: &mut SanitizerReport) {
    for step in &mut plan.steps {
        let mut kept = Vec::new();
        let mut readiness_note_needed = false;
        let mut existing = step.verify.iter().cloned().collect::<BTreeSet<_>>();
        let original_verify = std::mem::take(&mut step.verify);
        for command in original_verify {
            let diagnosis = diagnose_verify_command(&command);
            if diagnosis.violation != Some(VerifyCommandViolationKind::SetupOrDevServer) {
                kept.push(command);
                continue;
            }
            report.removed_commands.push(SanitizedCommandRecord {
                step_id: step.id.clone(),
                command: command.clone(),
                reason: diagnosis.reason.clone().unwrap_or_else(|| {
                    "verify command may not perform setup or start a dev server".to_string()
                }),
            });
            if command_implies_browser_readiness(&diagnosis.normalized) {
                readiness_note_needed = true;
                continue;
            }
            for candidate in expected_path_file_checks(step) {
                if existing.insert(candidate.clone()) {
                    kept.push(candidate.clone());
                    report
                        .substituted_commands
                        .push(SanitizedSubstitutionRecord {
                            step_id: step.id.clone(),
                            removed_command: command.clone(),
                            substituted_command: candidate,
                        });
                }
            }
        }
        step.verify = kept;
        if readiness_note_needed && append_browser_readiness_note(step) {
            report.instruction_notes.push(SanitizedInstructionNote {
                step_id: step.id.clone(),
                note: BROWSER_READINESS_NOTE.to_string(),
            });
        }
    }
}

fn retype_manifest_step_if_needed(plan: &mut StepPlan, report: &mut SanitizerReport) {
    if plan
        .steps
        .iter()
        .any(|step| step.step_kind() == StepKind::Setup)
    {
        return;
    }
    let Some(step) = plan
        .steps
        .iter_mut()
        .find(|step| step_creates_dependency_manifest(step))
    else {
        return;
    };
    let from_kind = step.kind.clone();
    if from_kind == "setup" {
        return;
    }
    step.kind = "setup".to_string();
    report.retyped_steps.push(SanitizedRetypeRecord {
        step_id: step.id.clone(),
        from_kind,
        to_kind: "setup".to_string(),
        reason: "dependency manifest creation defines the setup boundary".to_string(),
    });
}

fn move_dependency_order_commands(
    plan: &mut StepPlan,
    workspace_root: Option<&Path>,
    report: &mut SanitizerReport,
) {
    loop {
        let offenses = diagnose_step_plan_dependency_order(plan, workspace_root);
        let Some(offense) = offenses.into_iter().next() else {
            break;
        };
        if offense.kind != VerifyDependencyOrderViolationKind::RequiresSetup {
            break;
        }
        let Some(command) = remove_verify_command_at(
            plan,
            offense.step_index,
            offense.command_index,
            &offense.command,
        ) else {
            break;
        };
        let Some(target_index) = dependency_verify_target_index(plan, offense.step_index) else {
            report.dropped_commands.push(SanitizedCommandRecord {
                step_id: offense.step_id,
                command,
                reason: offense.message,
            });
            continue;
        };
        let from_step_id = offense.step_id;
        let to_step_id = plan.steps[target_index].id.clone();
        if append_verify_command(&mut plan.steps[target_index], command.clone()) {
            report.moved_commands.push(SanitizedMoveRecord {
                from_step_id,
                to_step_id,
                command,
                reason: offense.message,
            });
        } else {
            report.dropped_commands.push(SanitizedCommandRecord {
                step_id: from_step_id,
                command,
                reason: "dependency verify command already exists at or after setup boundary"
                    .to_string(),
            });
        }
    }
}

fn remove_verify_command_at(
    plan: &mut StepPlan,
    step_index: usize,
    command_index: usize,
    command: &str,
) -> Option<String> {
    let step = plan.steps.get_mut(step_index)?;
    if step
        .verify
        .get(command_index)
        .is_some_and(|value| value == command)
    {
        return Some(step.verify.remove(command_index));
    }
    let index = step.verify.iter().position(|value| value == command)?;
    Some(step.verify.remove(index))
}

fn dependency_verify_target_index(plan: &StepPlan, source_index: usize) -> Option<usize> {
    let boundary = setup_boundary_index(plan)?;
    let start = source_index.max(boundary.saturating_add(1));
    (start..plan.steps.len())
        .find(|index| verify_target_accepts_dependency_command(&plan.steps[*index]))
}

fn setup_boundary_index(plan: &StepPlan) -> Option<usize> {
    plan.steps
        .iter()
        .position(|step| step.step_kind() == StepKind::Setup)
}

fn verify_target_accepts_dependency_command(step: &PlanStep) -> bool {
    !matches!(
        step.step_kind(),
        StepKind::Setup | StepKind::Inspect | StepKind::Report | StepKind::Unknown(_)
    )
}

fn append_verify_command(step: &mut PlanStep, command: String) -> bool {
    if diagnose_verify_command(&command).violation.is_some() {
        return false;
    }
    if step.verify.iter().any(|existing| existing == &command) {
        return false;
    }
    step.verify.push(command);
    true
}

fn expected_path_file_checks(step: &PlanStep) -> Vec<String> {
    step.expected_paths
        .iter()
        .filter_map(|path| {
            validate_workspace_relative(path).ok()?;
            let command = format!("test -f {path}");
            (diagnose_verify_command(&command).violation.is_none()).then_some(command)
        })
        .collect()
}

fn command_implies_browser_readiness(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.contains("localhost")
        || lower.contains("127.0.0.1")
        || lower.contains("[::1]")
        || lower.contains("npm run dev")
        || lower.contains("pnpm dev")
        || lower.contains("yarn dev")
        || lower.contains("next dev")
        || lower.contains("vite --host")
        || lower.contains("vite --port")
}

fn append_browser_readiness_note(step: &mut PlanStep) -> bool {
    if step.instruction.contains(BROWSER_READINESS_NOTE) {
        return false;
    }
    if !step.instruction.trim_end().ends_with('.') {
        step.instruction.push('.');
    }
    step.instruction.push(' ');
    step.instruction.push_str(BROWSER_READINESS_NOTE);
    true
}

fn normalize_empty_verify_steps(plan: &mut StepPlan, report: &mut SanitizerReport) {
    if report.is_empty() {
        return;
    }
    for step in &mut plan.steps {
        if step.step_kind() == StepKind::Verify
            && step.verify.is_empty()
            && step.expected_paths.is_empty()
        {
            let from_kind = step.kind.clone();
            step.kind = "inspect".to_string();
            report.retyped_steps.push(SanitizedRetypeRecord {
                step_id: step.id.clone(),
                from_kind,
                to_kind: "inspect".to_string(),
                reason: "verify step became empty after deterministic command relocation"
                    .to_string(),
            });
        }
    }
}

fn step_creates_dependency_manifest(step: &PlanStep) -> bool {
    step.expected_paths.iter().any(|path| {
        matches!(
            path.as_str(),
            "package.json" | "Cargo.toml" | "pyproject.toml"
        )
    })
}

fn dedupe_verify_commands(plan: &mut StepPlan) {
    for step in &mut plan.steps {
        let mut seen = BTreeSet::new();
        step.verify.retain(|command| seen.insert(command.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::lint::lint_step_plan_report_with_workspace;

    #[test]
    fn sanitizer_removes_setup_and_dev_server_verify_and_retypes_manifest_step() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = StepPlan {
            goal: "Scaffold a Next.js project".to_string(),
            steps: vec![
                PlanStep {
                    id: "create-manifest".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Create package.json for the Next.js app".to_string(),
                    expected_paths: vec!["package.json".to_string()],
                    verify: vec!["npm install".to_string()],
                },
                PlanStep {
                    id: "create-page".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Create the app route".to_string(),
                    expected_paths: vec!["src/app/page.tsx".to_string()],
                    verify: vec!["npm run dev & curl http://localhost:3011".to_string()],
                },
            ],
        };

        let report = sanitize_step_plan_against_policy(&mut plan, Some(dir.path()));

        assert_eq!(report.removed_commands.len(), 2);
        assert_eq!(report.substituted_commands.len(), 1);
        assert_eq!(plan.steps[0].kind, "setup");
        assert_eq!(plan.steps[0].verify, vec!["test -f package.json"]);
        assert!(plan.steps[1].verify.is_empty());
        assert!(
            plan.steps[1]
                .instruction
                .contains("Browser readiness is verified by the runtime")
        );
        assert!(
            lint_step_plan_report_with_workspace(&plan, Some(dir.path())).is_pass(),
            "{plan:?}"
        );
    }

    #[test]
    fn sanitizer_moves_dependency_verify_after_setup_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = StepPlan {
            goal: "Create a Next.js app".to_string(),
            steps: vec![
                PlanStep {
                    id: "precheck".to_string(),
                    kind: "verify".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Verify Next can be loaded".to_string(),
                    expected_paths: Vec::new(),
                    verify: vec![r#"node -e "require('next/package.json')""#.to_string()],
                },
                PlanStep {
                    id: "setup-project".to_string(),
                    kind: "setup".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Create package.json with dependencies".to_string(),
                    expected_paths: vec!["package.json".to_string()],
                    verify: Vec::new(),
                },
                PlanStep {
                    id: "create-page".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Create src/app/page.tsx".to_string(),
                    expected_paths: vec!["src/app/page.tsx".to_string()],
                    verify: Vec::new(),
                },
            ],
        };

        let report = sanitize_step_plan_against_policy(&mut plan, Some(dir.path()));

        assert_eq!(report.moved_commands.len(), 1);
        assert!(plan.steps[0].verify.is_empty());
        assert_eq!(
            plan.steps[2].verify,
            vec![r#"node -e "require('next/package.json')""#]
        );
        assert!(
            lint_step_plan_report_with_workspace(&plan, Some(dir.path())).is_pass(),
            "{plan:?}"
        );
    }

    #[test]
    fn sanitizer_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = StepPlan {
            goal: "Scaffold a Next.js project".to_string(),
            steps: vec![PlanStep {
                id: "create-manifest".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create package.json".to_string(),
                expected_paths: vec!["package.json".to_string()],
                verify: vec!["npm install".to_string()],
            }],
        };
        sanitize_step_plan_against_policy(&mut plan, Some(dir.path()));
        let once = plan.clone();

        let second = sanitize_step_plan_against_policy(&mut plan, Some(dir.path()));

        assert!(second.is_empty());
        assert_eq!(plan, once);
    }

    #[test]
    fn sanitizer_does_not_alter_valid_plan() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = StepPlan {
            goal: "Create README".to_string(),
            steps: vec![PlanStep {
                id: "create-readme".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create README.md".to_string(),
                expected_paths: vec!["README.md".to_string()],
                verify: vec!["test -f README.md".to_string()],
            }],
        };
        let before = serde_json::to_string(&plan).unwrap();

        let report = sanitize_step_plan_against_policy(&mut plan, Some(dir.path()));

        assert!(report.is_empty());
        assert_eq!(serde_json::to_string(&plan).unwrap(), before);
    }
}
