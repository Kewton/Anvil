use std::collections::BTreeSet;
use std::path::Path;

use crate::planner::lint::{
    VerifyDependencyOrderViolationKind, diagnose_step_plan_dependency_order,
};
use crate::planner::step_plan::{PlanStep, StepKind, StepPlan};
use crate::planner::verify::{
    VerifyCommandViolationKind, diagnose_verify_command, normalize_verify_command_for_oracle_repair,
};
use crate::tools::path_guard::validate_workspace_relative;

const BROWSER_READINESS_NOTE: &str =
    "Browser readiness is verified by the runtime at final acceptance.";
const STEP_PLAN_GOAL_LINT_LIMIT_CHARS: usize = 4_000;
const SANITIZED_GOAL_MAX_CHARS: usize = 600;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SanitizerReport {
    pub goal_truncations: Vec<SanitizedGoalTruncationRecord>,
    pub normalized_commands: Vec<SanitizedCommandNormalizationRecord>,
    pub removed_commands: Vec<SanitizedCommandRecord>,
    pub substituted_commands: Vec<SanitizedSubstitutionRecord>,
    pub moved_commands: Vec<SanitizedMoveRecord>,
    pub dropped_commands: Vec<SanitizedCommandRecord>,
    pub retyped_steps: Vec<SanitizedRetypeRecord>,
    pub instruction_notes: Vec<SanitizedInstructionNote>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedGoalTruncationRecord {
    pub kind: String,
    pub original_len: usize,
    pub new_len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedCommandRecord {
    pub step_id: String,
    pub command: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedCommandNormalizationRecord {
    pub step_id: String,
    pub original_command: String,
    pub normalized_command: String,
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
        self.goal_truncations.is_empty()
            && self.normalized_commands.is_empty()
            && self.removed_commands.is_empty()
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
    normalize_oversized_goal(plan, &mut report);
    normalize_repairable_verify_commands(plan, &mut report);
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

fn normalize_repairable_verify_commands(plan: &mut StepPlan, report: &mut SanitizerReport) {
    for step in &mut plan.steps {
        for command in &mut step.verify {
            let Some(repair) = normalize_verify_command_for_oracle_repair(command) else {
                continue;
            };
            if repair.normalized == *command {
                continue;
            }
            let original = std::mem::replace(command, repair.normalized.clone());
            report
                .normalized_commands
                .push(SanitizedCommandNormalizationRecord {
                    step_id: step.id.clone(),
                    original_command: original,
                    normalized_command: repair.normalized,
                    reason: repair.reason,
                });
        }
    }
}

fn normalize_oversized_goal(plan: &mut StepPlan, report: &mut SanitizerReport) {
    let original_len = plan.goal.chars().count();
    if original_len <= STEP_PLAN_GOAL_LINT_LIMIT_CHARS {
        return;
    }

    let normalized = normalized_goal_summary(&plan.goal);
    let new_len = normalized.chars().count();
    plan.goal = normalized;
    report.goal_truncations.push(SanitizedGoalTruncationRecord {
        kind: "goal_truncated".to_string(),
        original_len,
        new_len,
    });
}

fn normalized_goal_summary(goal: &str) -> String {
    let candidate = phase_task_excerpt(goal)
        .or_else(|| first_non_guidance_line(goal))
        .unwrap_or_else(|| "Complete the current phase.".to_string());
    let stripped = strip_echoed_guidance_sections(&candidate);
    let compact = collapse_whitespace(if stripped.trim().is_empty() {
        candidate.trim()
    } else {
        stripped.trim()
    });
    let bounded = truncate_at_sentence_or_line_boundary(&compact, SANITIZED_GOAL_MAX_CHARS);
    if bounded.trim().is_empty() {
        "Complete the current phase.".to_string()
    } else {
        bounded
    }
}

fn phase_task_excerpt(goal: &str) -> Option<String> {
    goal.lines()
        .find_map(|line| line.trim().strip_prefix("Phase task:"))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            truncate_at_sentence_or_line_boundary(
                &first_sentence_or_line(line),
                SANITIZED_GOAL_MAX_CHARS,
            )
        })
}

fn first_non_guidance_line(goal: &str) -> Option<String> {
    strip_echoed_guidance_sections(goal)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !is_echoed_guidance_line(line))
        .map(|line| truncate_at_sentence_or_line_boundary(line, SANITIZED_GOAL_MAX_CHARS))
}

fn strip_echoed_guidance_sections(text: &str) -> String {
    let mut out = Vec::new();
    let mut skipping_guidance_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if starts_guidance_section(trimmed) {
            skipping_guidance_section = true;
            continue;
        }
        if skipping_guidance_section {
            if trimmed.is_empty() {
                skipping_guidance_section = false;
            }
            continue;
        }
        if is_echoed_guidance_line(trimmed) {
            continue;
        }
        out.push(line);
    }
    out.join("\n")
}

fn starts_guidance_section(line: &str) -> bool {
    matches!(
        line,
        "Profile runtime contract:"
            | "Profile contract:"
            | "Route-bound implementation constraint:"
            | "Deterministic verification preference:"
    ) || line.starts_with("Unmet final requirements")
        || line.starts_with("Requested features")
}

fn is_echoed_guidance_line(line: &str) -> bool {
    if starts_guidance_section(line) {
        return true;
    }
    if !line.starts_with("- ") {
        return false;
    }
    let lower = line[2..].to_ascii_lowercase();
    lower.starts_with("profile ")
        || lower.starts_with("preserve ")
        || lower.starts_with("keep ")
        || lower.starts_with("prefer ")
        || lower.starts_with("do not ")
        || lower.starts_with("if ")
        || lower.starts_with("browser readiness ")
        || lower.starts_with("close these requirements ")
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn first_sentence_or_line(text: &str) -> String {
    for (idx, ch) in text.char_indices() {
        if matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | '\n') {
            let end = idx + ch.len_utf8();
            let candidate = text[..end].trim();
            if !candidate.is_empty() {
                return candidate.to_string();
            }
        }
    }
    text.trim().to_string()
}

fn truncate_at_sentence_or_line_boundary(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.trim().to_string();
    }
    let mut best = None;
    for (idx, ch) in text.char_indices() {
        let end = idx + ch.len_utf8();
        let chars = text[..end].chars().count();
        if chars > max_chars {
            break;
        }
        if matches!(ch, '.' | '!' | '?' | '。' | '！' | '？' | '\n') {
            best = Some(end);
        }
    }
    if let Some(end) = best {
        let candidate = text[..end].trim();
        if !candidate.is_empty() {
            return candidate.to_string();
        }
    }
    text.chars()
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
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

    fn valid_artifact_plan(goal: String) -> StepPlan {
        StepPlan {
            goal,
            steps: vec![PlanStep {
                id: "create-page".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create src/app/page.tsx for the requested phase.".to_string(),
                expected_paths: vec!["src/app/page.tsx".to_string()],
                verify: vec!["test -f src/app/page.tsx".to_string()],
            }],
        }
    }

    fn uat_shaped_echoed_phase_prompt() -> String {
        format!(
            "Original ultra goal: {}\n\
Profile: nextjs\n\
Style: default\n\
Intent: create\n\
Phase id: arcade-ui-and-local-storage\n\
Phase task: Build the arcade UI and local storage persistence. Add details in the steps.\n\n\
Workspace snapshot:\n- none\n\n\
Unmet final requirements from earlier phases:\n- restart_or_recoverable_state_evidence\n- interaction_evidence\n\n\
Requested features not yet detected: keyboard, score, collision, wave, audio, particles, highscore\n\n\
Profile runtime contract:\n- Preserve the workspace as a real Next.js app.\n- Keep next/react/react-dom dependencies in package.json.\n- Do not treat scaffold-only output as complete.\n\n{}",
            "Create a polished canvas arcade game with persistent progress. ".repeat(80),
            "Carry forward evidence and profile constraints. ".repeat(120)
        )
    }

    #[test]
    fn sanitizer_truncates_echoed_phase_prompt_goal_before_lint() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = valid_artifact_plan(uat_shaped_echoed_phase_prompt());
        assert!(plan.goal.chars().count() > STEP_PLAN_GOAL_LINT_LIMIT_CHARS);

        let report = sanitize_step_plan_against_policy(&mut plan, Some(dir.path()));

        assert_eq!(report.goal_truncations.len(), 1);
        assert_eq!(report.goal_truncations[0].kind, "goal_truncated");
        assert!(report.goal_truncations[0].original_len > STEP_PLAN_GOAL_LINT_LIMIT_CHARS);
        assert!(report.goal_truncations[0].new_len <= SANITIZED_GOAL_MAX_CHARS);
        assert_eq!(
            plan.goal,
            "Build the arcade UI and local storage persistence."
        );
        assert!(!plan.goal.contains("Unmet final requirements"));
        assert!(!plan.goal.contains("Requested features"));
        assert!(!plan.goal.contains("Profile runtime contract"));
        assert!(
            lint_step_plan_report_with_workspace(&plan, Some(dir.path())).is_pass(),
            "{plan:?}"
        );
    }

    #[test]
    fn sanitizer_guarantees_goal_length_lint_cannot_fire() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = valid_artifact_plan("x".repeat(10_000));

        sanitize_step_plan_against_policy(&mut plan, Some(dir.path()));

        assert!(plan.goal.chars().count() <= SANITIZED_GOAL_MAX_CHARS);
        let lint = lint_step_plan_report_with_workspace(&plan, Some(dir.path()));
        assert!(
            !lint
                .errors
                .iter()
                .any(|err| err.message == "StepPlan goal is too long"),
            "{lint:?}"
        );
        assert!(lint.is_pass(), "{lint:?}");
    }

    #[test]
    fn goal_truncation_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = valid_artifact_plan(uat_shaped_echoed_phase_prompt());
        sanitize_step_plan_against_policy(&mut plan, Some(dir.path()));
        let once = plan.clone();

        let second = sanitize_step_plan_against_policy(&mut plan, Some(dir.path()));

        assert!(second.is_empty());
        assert_eq!(plan, once);
    }

    #[test]
    fn sanitizer_leaves_short_goal_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = valid_artifact_plan("Create README".to_string());
        let before = serde_json::to_string(&plan).unwrap();

        let report = sanitize_step_plan_against_policy(&mut plan, Some(dir.path()));

        assert!(report.is_empty());
        assert_eq!(serde_json::to_string(&plan).unwrap(), before);
    }

    #[test]
    fn sanitizer_normalizes_grep_dash_pattern_and_lint_passes_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = StepPlan {
            goal: "Check the package dev port".to_string(),
            steps: vec![PlanStep {
                id: "verify-port".to_string(),
                kind: "verify".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Verify the package script uses port 3011".to_string(),
                expected_paths: vec!["package.json".to_string()],
                verify: vec![r#"grep -q "-p 3011" package.json"#.to_string()],
            }],
        };

        let lint_before = lint_step_plan_report_with_workspace(&plan, Some(dir.path()));
        assert!(
            lint_before.errors.iter().any(|err| {
                err.category == "verify_policy"
                    && err
                        .message
                        .contains("grep pattern begins with '-' but command lacks `--` or `-e`")
            }),
            "{lint_before:?}"
        );
        let report = sanitize_step_plan_against_policy(&mut plan, Some(dir.path()));

        assert_eq!(report.normalized_commands.len(), 1);
        assert_eq!(
            report.normalized_commands[0].original_command,
            r#"grep -q "-p 3011" package.json"#
        );
        assert_eq!(
            plan.steps[0].verify,
            vec![r#"grep -q -- "-p 3011" package.json"#]
        );
        let lint_after = lint_step_plan_report_with_workspace(&plan, Some(dir.path()));
        assert!(lint_after.is_pass(), "{lint_after:?}");
        let second = sanitize_step_plan_against_policy(&mut plan, Some(dir.path()));
        assert!(second.is_empty(), "{second:?}");
        assert_eq!(
            plan.steps[0].verify,
            vec![r#"grep -q -- "-p 3011" package.json"#]
        );
    }

    #[test]
    fn sanitizer_prefers_json_parser_for_recognizable_package_script_grep() {
        let dir = tempfile::tempdir().unwrap();
        let mut plan = StepPlan {
            goal: "Check the package dev script".to_string(),
            steps: vec![PlanStep {
                id: "verify-dev-script".to_string(),
                kind: "verify".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Verify the package dev script".to_string(),
                expected_paths: vec!["package.json".to_string()],
                verify: vec![r#"grep -q "next dev -p 3011" package.json"#.to_string()],
            }],
        };

        let report = sanitize_step_plan_against_policy(&mut plan, Some(dir.path()));

        assert_eq!(report.normalized_commands.len(), 1);
        assert_eq!(
            plan.steps[0].verify,
            vec![
                r#"node -p "String(require('./package.json').scripts.dev).includes('next dev -p 3011') ? true : process.exit(1)""#
            ]
        );
        assert!(
            lint_step_plan_report_with_workspace(&plan, Some(dir.path())).is_pass(),
            "{plan:?}"
        );
    }

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
