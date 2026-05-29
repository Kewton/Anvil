use super::*;
use crate::logging::log_llm_event;
use crate::modes::plan_act::{PlanStage, TaskProfile};
use serde_json::Value;

impl Agent {
    pub(super) fn current_plan_contents(&self) -> Result<Option<String>, String> {
        let Some(path) = &self.session.mode_state.active_plan_path else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }
        let contents = std::fs::read_to_string(path)
            .map_err(|err| format!("failed to read plan {}: {err}", path.display()))?;
        Ok(Some(contents))
    }

    pub(super) fn persist_session(&mut self) -> Result<(), String> {
        self.session.active_root = if self.work_root == self.config.cwd {
            None
        } else {
            Some(self.work_root.clone())
        };
        self.session_store.save(&self.session)
    }

    pub(super) fn maybe_compact_session(&mut self, keep_tail: usize) -> bool {
        if !should_compact(
            &self.session.messages,
            self.config.context_budget,
            keep_tail,
        ) {
            return false;
        }

        if let Some(sidecar) = self.models.sidecar.clone() {
            compact_messages_with_strategy(&mut self.session.messages, keep_tail, |head| {
                self.client.summarize_conversation(&sidecar, head)
            })
            .unwrap_or_else(|_| compact_messages(&mut self.session.messages, keep_tail))
        } else {
            compact_messages(&mut self.session.messages, keep_tail)
        }
    }

    pub(super) fn maybe_compact_late_turn_session(
        &mut self,
        tool_calls_this_turn: usize,
        repo_edit_calls_this_turn: usize,
    ) -> bool {
        if !should_compact_late_turn(
            &self.session.messages,
            self.config.context_budget,
            tool_calls_this_turn,
            repo_edit_calls_this_turn,
        ) {
            return false;
        }

        self.maybe_compact_session(crate::agent::loop_run::LATE_TURN_KEEP_TAIL)
    }

    pub(super) fn ensure_plan_file(&self, plan_path: &Path) -> Result<(), String> {
        if plan_path.exists() {
            return Ok(());
        }
        if let Some(parent) = plan_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
        }
        std::fs::write(
            plan_path,
            "# Plan\n\n## Goal\n- \n\n## Constraints\n- \n\n## First Action\n- \n\n## Verification\n- \n",
        )
        .map_err(|err| format!("failed to create plan file {}: {err}", plan_path.display()))
    }

    pub(super) fn maybe_update_work_root(
        &mut self,
        name: &str,
        arguments: &serde_json::Value,
        result: &str,
    ) {
        let Some(new_root) = prompting::detect_scaffold_root(name, arguments, result) else {
            return;
        };
        if new_root == self.work_root || !new_root.is_dir() {
            return;
        }

        self.apply_scaffold_root(new_root);
    }

    pub(super) fn disable_native_tools_for_session(&mut self) {
        self.native_tools_enabled = false;
        self.session.native_tools_disabled = true;
        self.push_system_note(prompting::ToolProtocol::TaggedXml.parser_downgrade_notice());
    }

    fn apply_scaffold_root(&mut self, new_root: PathBuf) {
        self.work_root = new_root.clone();
        self.session.active_root = Some(new_root.clone());
        self.push_system_note(format!(
            "[Workspace Root Updated] Continue work inside {} and use relative paths from there.",
            new_root.display()
        ));
    }
}

pub(super) fn should_compact(
    messages: &[ConversationMessage],
    context_budget: usize,
    keep_tail: usize,
) -> bool {
    messages.len() > keep_tail + 4 || approximate_token_count(messages) > context_budget
}

pub(super) fn should_compact_late_turn(
    messages: &[ConversationMessage],
    context_budget: usize,
    tool_calls_this_turn: usize,
    repo_edit_calls_this_turn: usize,
) -> bool {
    if repo_edit_calls_this_turn == 0 && tool_calls_this_turn < 4 {
        return false;
    }

    messages.len() >= 16 || approximate_token_count(messages) > context_budget.saturating_mul(3) / 5
}

pub(super) fn is_native_tool_parser_failure(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("native tool parser failed")
        || lower.contains("unexpected end element")
        || lower.contains("unexpected eof")
}

pub(super) fn is_native_tool_transport_failure(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("ollama /api/chat failed: 5")
}

pub(super) fn is_tool_call_format_error(error: &str) -> bool {
    error
        .to_ascii_lowercase()
        .contains("tool call parser failed:")
}

pub(super) fn is_transport_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("failed to contact ollama chat api")
        || lower.contains("error sending request for url")
        || lower.contains("timed out")
        || lower.contains("connection reset")
        || lower.contains("connection refused")
        || lower.contains("broken pipe")
        || lower.contains("ollama /api/chat failed: 5")
        || lower.contains("ollama /api/chat failed: 429")
}

const PLAN_STAGE_ONE: &[&str] = &["Goal", "Constraints"];
const PLAN_STAGE_TWO: &[&str] = &["First Action", "Verification"];
const PLAN_STAGE_THREE: &[&str] = &[];
const QUALITY_BAR_ANCHOR_SECTIONS: &[&str] = &[
    "Goal",
    "Constraints",
    "First Action",
    "Execution Plan",
    "Verification",
    "Verification Plan",
];

pub(super) fn plan_stage_sections(stage: PlanStage) -> &'static [&'static str] {
    match stage {
        PlanStage::Stage1 => PLAN_STAGE_ONE,
        PlanStage::Stage2 => PLAN_STAGE_TWO,
        PlanStage::Stage3 => PLAN_STAGE_THREE,
        PlanStage::Ready => &[],
    }
}

fn join_plan_sections(sections: &[&str]) -> String {
    match sections {
        [] => String::new(),
        [one] => (*one).to_string(),
        [first, second] => format!("{first} and {second}"),
        _ => {
            let mut parts = sections[..sections.len() - 1]
                .iter()
                .map(|section| (*section).to_string())
                .collect::<Vec<_>>();
            let tail = sections[sections.len() - 1];
            parts.push(format!("and {tail}"));
            parts.join(", ")
        }
    }
}

fn task_scope_label(task_profile: TaskProfile) -> &'static str {
    match task_profile {
        TaskProfile::Coding => "implementation work",
        TaskProfile::Content => "content update",
        TaskProfile::Ui => "UI work",
        TaskProfile::Research => "research task",
        TaskProfile::Generic => "requested task",
    }
}

pub(super) fn plan_task_list(contents: &str, task_profile: TaskProfile) -> Vec<String> {
    let groups = [
        (
            PLAN_STAGE_ONE,
            format!(
                "Fill {} for the {}",
                join_plan_sections(PLAN_STAGE_ONE),
                task_scope_label(task_profile)
            ),
        ),
        (
            PLAN_STAGE_TWO,
            format!("Fill {}", join_plan_sections(PLAN_STAGE_TWO)),
        ),
        (
            PLAN_STAGE_THREE,
            format!("Fill {}", join_plan_sections(PLAN_STAGE_THREE)),
        ),
    ];
    let groups = groups
        .into_iter()
        .filter(|(sections, _)| !sections.is_empty())
        .collect::<Vec<_>>();

    let current_group = groups
        .iter()
        .position(|(sections, _)| {
            sections
                .iter()
                .any(|section| !plan_section_is_complete(contents, section))
        })
        .unwrap_or(groups.len());

    let mut tasks = Vec::new();
    for (index, (sections, label)) in groups.iter().enumerate() {
        let missing = sections
            .iter()
            .copied()
            .filter(|section| !plan_section_is_complete(contents, section))
            .collect::<Vec<_>>();
        let status = if missing.is_empty() {
            "[done]"
        } else if index == current_group {
            "[in progress]"
        } else {
            "[pending]"
        };
        let task = if missing.is_empty() {
            label.clone()
        } else {
            format!("Fill {}", join_plan_sections(&missing))
        };
        tasks.push(format!("{status} {task}"));
    }

    let approval_status = if current_group >= groups.len() {
        "[in progress]"
    } else {
        "[pending]"
    };
    tasks.push(format!(
        "{approval_status} Review the completed plan and approve execution"
    ));
    tasks
}

fn normalize_plan_heading(heading: &str) -> &str {
    match heading.trim() {
        "Next Step" | "First Step" | "Execution Plan" | "実行計画" | "実装計画"
        | "実装フェーズ" => "First Action",
        "Verification Plan" | "検証計画" => "Verification",
        "Risks/Fallbacks" => "Risks / Fallbacks",
        "リスク/フォールバック" | "リスク・フォールバック" | "リスク / フォールバック" => {
            "Risks / Fallbacks"
        }
        other => other,
    }
}

fn substantive_plan_lines<'a>(body: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| *line != "-")
        .filter(|line| *line != "1.")
        .filter(|line| *line != "2.")
        .filter(|line| *line != "3.")
        .filter(|line| {
            !matches!(
                *line,
                "1. First slice:" | "2. Next phases:" | "3. Review checkpoint:"
            )
        })
}

fn plan_section_body(contents: &str, section: &str) -> Option<String> {
    let mut current_heading: Option<&str> = None;
    let mut collected = Vec::new();

    for raw_line in contents.lines() {
        let trimmed = raw_line.trim();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            current_heading = Some(normalize_plan_heading(heading));
            continue;
        }

        if current_heading == Some(section) {
            collected.push(raw_line);
        }
    }

    (!collected.is_empty()).then(|| collected.join("\n"))
}

fn plan_section_is_substantive(contents: &str, section: &str) -> bool {
    plan_section_body(contents, section)
        .map(|body| substantive_plan_lines(&body).count() > 0)
        .unwrap_or(false)
}

fn plan_section_is_complete(contents: &str, section: &str) -> bool {
    if !plan_section_is_substantive(contents, section) {
        return false;
    }
    if section == "Quality Bar" {
        return quality_bar_has_repo_specific_anchor(contents);
    }
    true
}

fn quality_bar_has_repo_specific_anchor(contents: &str) -> bool {
    let Some(body) = plan_section_body(contents, "Quality Bar") else {
        return false;
    };
    let anchors = repo_specific_anchor_terms(contents);
    if anchors.is_empty() {
        return false;
    }
    substantive_plan_lines(&body).any(|line| {
        let lowered = line.to_ascii_lowercase();
        lowered.contains('`')
            || lowered.contains('/')
            || [
                ".md", ".rs", ".ts", ".tsx", ".js", ".jsx", ".json", ".toml", ".py",
            ]
            .iter()
            .any(|needle| lowered.contains(needle))
            || tokenize_plan_terms(line)
                .into_iter()
                .any(|token| anchors.contains(&token))
    })
}

fn repo_specific_anchor_terms(contents: &str) -> std::collections::HashSet<String> {
    let mut anchors = std::collections::HashSet::new();
    for section in QUALITY_BAR_ANCHOR_SECTIONS {
        let Some(body) = plan_section_body(contents, section) else {
            continue;
        };
        for line in substantive_plan_lines(&body) {
            for token in tokenize_plan_terms(line) {
                anchors.insert(token);
            }
        }
    }
    anchors
}

fn tokenize_plan_terms(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut previous_was_lower = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if ch.is_ascii_uppercase() && previous_was_lower && !current.is_empty() {
                push_plan_term(&mut tokens, &mut current);
            }
            current.push(ch.to_ascii_lowercase());
            previous_was_lower = ch.is_ascii_lowercase();
        } else {
            push_plan_term(&mut tokens, &mut current);
            previous_was_lower = false;
        }
    }
    push_plan_term(&mut tokens, &mut current);
    tokens.sort();
    tokens.dedup();
    tokens
}

fn push_plan_term(tokens: &mut Vec<String>, current: &mut String) {
    if current.len() >= 4 && !is_generic_plan_term(current) {
        tokens.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

fn is_generic_plan_term(token: &str) -> bool {
    matches!(
        token,
        "that"
            | "this"
            | "with"
            | "from"
            | "into"
            | "keep"
            | "preserve"
            | "result"
            | "quality"
            | "plan"
            | "phases"
            | "phase"
            | "review"
            | "checkpoint"
            | "criteria"
            | "deliverables"
            | "constraints"
            | "execution"
            | "verification"
            | "fallback"
            | "fallbacks"
            | "risk"
            | "risks"
            | "concrete"
            | "useful"
            | "clear"
            | "clearer"
            | "generic"
            | "specific"
            | "reader"
            | "maintainer"
            | "current"
            | "existing"
            | "content"
            | "section"
            | "sections"
            | "file"
            | "files"
            | "repo"
            | "repository"
    )
}

pub(super) fn plan_missing_sections(contents: &str) -> Vec<&'static str> {
    PLAN_STAGE_ONE
        .iter()
        .chain(PLAN_STAGE_TWO.iter())
        .chain(PLAN_STAGE_THREE.iter())
        .copied()
        .filter(|section| !plan_section_is_complete(contents, section))
        .collect()
}

pub(super) fn plan_next_stage_sections(contents: &str) -> Vec<&'static str> {
    for stage in [PLAN_STAGE_ONE, PLAN_STAGE_TWO] {
        let missing = stage
            .iter()
            .copied()
            .filter(|section| !plan_section_is_substantive(contents, section))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return missing;
        }
    }
    Vec::new()
}

pub(super) fn current_plan_stage(contents: &str) -> PlanStage {
    if PLAN_STAGE_ONE
        .iter()
        .any(|section| !plan_section_is_complete(contents, section))
    {
        PlanStage::Stage1
    } else if PLAN_STAGE_TWO
        .iter()
        .any(|section| !plan_section_is_complete(contents, section))
    {
        PlanStage::Stage2
    } else if PLAN_STAGE_THREE
        .iter()
        .any(|section| !plan_section_is_complete(contents, section))
    {
        PlanStage::Stage3
    } else {
        PlanStage::Ready
    }
}

pub(super) fn plan_stage_exploration_budget(stage: PlanStage) -> usize {
    match stage {
        PlanStage::Stage1 => 1,
        PlanStage::Stage2 => 1,
        PlanStage::Stage3 => 0,
        PlanStage::Ready => 0,
    }
}

pub(super) fn plan_is_substantive(contents: &str) -> bool {
    plan_missing_sections(contents).is_empty()
}

pub(super) fn plan_is_approval_ready(contents: &str) -> bool {
    ["Goal", "Constraints", "First Action", "Verification"]
        .into_iter()
        .all(|section| plan_section_is_complete(contents, section))
}

fn plan_needs_stage_three_fallback(contents: &str) -> bool {
    let missing = plan_missing_sections(contents);
    !missing.is_empty()
        && missing
            .iter()
            .all(|section| PLAN_STAGE_THREE.contains(section))
        && PLAN_STAGE_ONE
            .iter()
            .chain(PLAN_STAGE_TWO.iter())
            .all(|section| plan_section_is_substantive(contents, section))
        && PLAN_STAGE_THREE
            .iter()
            .any(|section| plan_section_is_substantive(contents, section))
}

pub(crate) fn extract_first_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;
    for (offset, ch) in raw[start..].char_indices() {
        if escape {
            escape = false;
            continue;
        }
        match ch {
            '\\' if in_string => escape = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&raw[start..start + offset + ch.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_stage_three_fallback_decision(raw: &str) -> Option<bool> {
    let body = extract_first_json_object(raw.trim())?;
    let value: Value = serde_json::from_str(body).ok()?;
    value
        .get("contains_stage_three_sections")
        .and_then(Value::as_bool)
}

pub(super) fn plan_act_summary(contents: &str) -> String {
    let mut parts = Vec::new();
    for section in ["Goal", "Constraints", "First Action", "Verification"] {
        if let Some(body) = plan_section_body(contents, section) {
            let lines = substantive_plan_lines(&body)
                .take(3)
                .map(|line| line.to_string())
                .collect::<Vec<_>>();
            if !lines.is_empty() {
                parts.push(format!("## {section}\n{}", lines.join("\n")));
            }
        }
    }
    parts.join("\n\n")
}

pub(super) fn format_tool_error(err: &str) -> String {
    if err.starts_with("Error:") {
        err.to_string()
    } else {
        format!("Error: {err}")
    }
}

impl Agent {
    pub(super) fn refresh_plan_stage(&mut self) -> Result<Option<PlanStage>, String> {
        if self.session.mode_state.mode != ExecutionMode::Plan {
            return Ok(None);
        }
        let Some(contents) = self.current_plan_contents()? else {
            return Ok(None);
        };
        let previous_stage = self.session.mode_state.plan_stage;
        let stage = current_plan_stage(&contents);
        self.session.mode_state.plan_stage = stage;
        if stage != previous_stage {
            log_llm_event(
                "agent.plan.stage_changed",
                serde_json::json!({
                    "session_id": self.session_store.session_id(),
                    "from": previous_stage.as_str(),
                    "to": stage.as_str(),
                    "next_sections": plan_next_stage_sections(&contents),
                    "missing_sections": plan_missing_sections(&contents),
                }),
            );
        }
        Ok(Some(stage))
    }

    fn plan_stage_three_fallback_matches(&self, contents: &str) -> bool {
        if !plan_needs_stage_three_fallback(contents) {
            return false;
        }
        let Some(sidecar) = self.models.sidecar.as_ref() else {
            return false;
        };

        let messages = vec![
            ConversationMessage::system(
                "You verify whether a plan already contains the substance of three sections. Reply with JSON only in this shape: {\"contains_stage_three_sections\":true|false}. Return true only if the plan substantially includes all of these: Execution Plan, Verification Plan, and Risks/Fallbacks, even when the headings are written in another language or alternate wording."
                    .to_string(),
            ),
            ConversationMessage::user(format!(
                "Decide whether this plan already contains the substance of Execution Plan, Verification Plan, and Risks/Fallbacks.\n\nPlan:\n{contents}"
            )),
        ];

        match self
            .client
            .classify_stage_three_fallback(sidecar, &messages)
        {
            Ok(reply) => parse_stage_three_fallback_decision(&reply.content).unwrap_or(false),
            Err(_) => false,
        }
    }

    pub(super) fn plan_is_substantive_with_fallback(&self, contents: &str) -> bool {
        plan_is_substantive(contents) || self.plan_stage_three_fallback_matches(contents)
    }

    pub(super) fn plan_is_approval_ready_with_fallback(&self, contents: &str) -> bool {
        plan_is_approval_ready(contents) || self.plan_stage_three_fallback_matches(contents)
    }
}

/// Extract bullet items under the `## Constraints` section of a plan
/// markdown document. Trims leading `- ` markers and skips empty
/// placeholders. Used by `refresh_working_memory` to keep the
/// per-prompt constraint list in sync with the live plan.
pub(super) fn extract_plan_constraints(contents: &str) -> Vec<String> {
    let mut in_constraints = false;
    let mut lines = Vec::new();
    for raw_line in contents.lines() {
        let trimmed = raw_line.trim();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            in_constraints = heading.trim() == "Constraints";
            continue;
        }
        if !in_constraints {
            continue;
        }
        if trimmed.is_empty() || trimmed == "-" {
            continue;
        }
        let cleaned = trimmed.trim_start_matches("- ").trim().to_string();
        if !cleaned.is_empty() {
            lines.push(cleaned);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::{
        current_plan_stage, is_native_tool_transport_failure, plan_act_summary,
        plan_is_approval_ready, plan_is_substantive, plan_missing_sections,
        plan_needs_stage_three_fallback, plan_next_stage_sections, plan_stage_exploration_budget,
        plan_stage_sections, plan_task_list,
    };
    use crate::modes::plan_act::{PlanStage, TaskProfile};

    const TEMPLATE: &str = "# Plan

## Goal
- 

## Constraints
- 

## Deliverables
- 

## Acceptance Criteria
- 

## Quality Bar
- 

## Execution Plan
1. First slice:
2. Next phases:
3. Review checkpoint:

## Verification Plan
- 

## Risks / Fallbacks
- 
";

    #[test]
    fn incomplete_template_reports_stage_one_missing() {
        assert_eq!(
            plan_next_stage_sections(TEMPLATE),
            vec!["Goal", "Constraints"]
        );
        assert!(!plan_is_substantive(TEMPLATE));
    }

    #[test]
    fn plan_task_list_uses_actual_missing_sections() {
        let tasks = plan_task_list(TEMPLATE, TaskProfile::Coding);
        assert_eq!(tasks[0], "[in progress] Fill Goal and Constraints");
        assert_eq!(tasks[1], "[pending] Fill First Action and Verification");
        assert_eq!(
            tasks[2],
            "[pending] Review the completed plan and approve execution"
        );
    }

    #[test]
    fn partially_filled_plan_advances_to_next_stage() {
        let contents = TEMPLATE
            .replace("## Goal\n- ", "## Goal\n- Improve the README")
            .replace(
                "## Constraints\n- ",
                "## Constraints\n- Keep the current content",
            )
            .replace(
                "## Deliverables\n- ",
                "## Deliverables\n- Updated README plan",
            );
        assert_eq!(
            plan_next_stage_sections(&contents),
            vec!["First Action", "Verification"]
        );
        assert_eq!(current_plan_stage(&contents), PlanStage::Stage2);
    }

    #[test]
    fn fully_filled_plan_is_substantive() {
        let contents = "# Plan

## Goal
- Improve the README in three stages.

## Constraints
- Preserve existing content.

## Deliverables
- A plan, a new heading, and a verification pass.

## Acceptance Criteria
- The new heading is present and the original text remains.

## Quality Bar
- The new section adds concrete value and does not read like filler.

## Execution Plan
1. First slice: define the README goal and heading to add.
2. Next phases: update the README and review the result.
3. Review checkpoint: confirm the final content before approval.

## Verification Plan
- Read the README after editing and confirm the expected heading exists.

## Risks / Fallbacks
- If the heading is unclear, revise the plan before execution.
";
        assert!(plan_missing_sections(contents).is_empty());
        assert!(plan_is_substantive(contents));
        assert_eq!(current_plan_stage(contents), PlanStage::Ready);
    }

    #[test]
    fn plan_act_summary_uses_key_sections_only() {
        let contents = "# Plan

## Goal
- Improve the README with clearer guidance.

## Constraints
- Keep the existing intro.

## Deliverables
- Better README text.

## Acceptance Criteria
- Add a specific usage section.

## Quality Bar
- Avoid vague filler text.
- Prefer concrete reader value.

## Execution Plan
1. First slice: add a focused usage section.
2. Next phases: tighten wording.
3. Review checkpoint: re-read for usefulness.

## Verification Plan
- Re-read README for clarity.

## Risks / Fallbacks
- If wording is vague, rewrite it.
";
        let summary = plan_act_summary(contents);
        assert!(summary.contains("## Goal"));
        assert!(summary.contains("## Constraints"));
        assert!(summary.contains("## First Action"));
        assert!(summary.contains("## Verification"));
        assert!(!summary.contains("## Deliverables"));
        assert!(!summary.contains("## Acceptance Criteria"));
    }

    #[test]
    fn approval_ready_allows_risks_to_be_filled_later() {
        let contents = "# Plan

## Goal
- Improve the README.

## Constraints
- Keep the existing sections.

## Deliverables
- Updated README plan.

## Acceptance Criteria
- Adds a useful new section.

## Quality Bar
- Anchor the quality bar to README.md readability.

## Execution Plan
1. Add the heading.
2. Add supporting content.
3. Review readability.

## Verification Plan
- Re-read the file and confirm the section is useful.

## Risks / Fallbacks
- 
";
        assert!(plan_is_approval_ready(contents));
    }

    #[test]
    fn stage_three_fallback_skips_placeholder_only_sections() {
        let contents = "# Plan

## Goal
- Build the feature.

## Constraints
- Keep the repo shape.

## Deliverables
- A runnable feature.

## Acceptance Criteria
- The first slice works.

## Quality Bar
- Anchor polish to `src/app/page.tsx`.

## Execution Plan
1. First slice:
2. Next phases:
3. Review checkpoint:

## Verification Plan
- 

## Risks / Fallbacks
- 
";
        assert!(!plan_needs_stage_three_fallback(contents));
    }

    #[test]
    fn stage_three_fallback_still_allows_partial_real_stage_three_content() {
        let contents = "# Plan

## Goal
- Build the feature.

## Constraints
- Keep the repo shape.

## Deliverables
- A runnable feature.

## Acceptance Criteria
- The first slice works.

## Quality Bar
- Anchor polish to `src/app/page.tsx`.

## Execution Plan
1. First slice: wire the main game loop in `src/app/page.tsx`.
2. Next phases:
3. Review checkpoint:

## Verification Plan
- 

## Risks / Fallbacks
- 
";
        assert!(!plan_needs_stage_three_fallback(contents));
    }

    #[test]
    fn generic_quality_bar_no_longer_blocks_minimal_plan() {
        let contents = "# Plan

## Goal
- Improve the README title and introduction.

## Constraints
- Keep the README structure stable.

## Deliverables
- A revised README plan.

## Acceptance Criteria
- README has a new title and short introduction.

## Quality Bar
- The result is clear and useful.

## Execution Plan
1. Update the README title.
2. Add a short introduction.
3. Review for clarity.

## Verification Plan
- Re-read the updated file.

## Risks / Fallbacks
- Revisit wording if the introduction is vague.
";
        assert_eq!(current_plan_stage(contents), PlanStage::Ready);
        assert!(plan_is_approval_ready(contents));
        assert!(plan_missing_sections(contents).is_empty());
    }

    #[test]
    fn repo_specific_quality_bar_is_approval_ready() {
        let contents = "# Plan

## Goal
- Improve the README title and introduction.

## Constraints
- Keep the README structure stable.

## Deliverables
- A revised README plan.

## Acceptance Criteria
- README has a new title and short introduction.

## Quality Bar
- The README introduction reflects the current repo scope instead of generic filler.
- The README title and intro stay aligned with the existing README sections.

## Execution Plan
1. Update the README title.
2. Add a short introduction.
3. Review for clarity.

## Verification Plan
- Re-read README.md and confirm the new introduction fits the existing sections.

## Risks / Fallbacks
- Revisit wording if the introduction is vague.
";
        assert_eq!(current_plan_stage(contents), PlanStage::Ready);
        assert!(plan_is_approval_ready(contents));
    }

    #[test]
    fn plan_stage_budget_is_tight_after_stage_one() {
        assert_eq!(plan_stage_exploration_budget(PlanStage::Stage1), 1);
        assert_eq!(plan_stage_exploration_budget(PlanStage::Stage2), 1);
        assert_eq!(plan_stage_exploration_budget(PlanStage::Stage3), 0);
        assert_eq!(plan_stage_exploration_budget(PlanStage::Ready), 0);
        assert_eq!(
            plan_stage_sections(PlanStage::Stage2),
            &["First Action", "Verification"]
        );
    }

    #[test]
    fn detects_native_tool_transport_failures() {
        assert!(is_native_tool_transport_failure(
            "Ollama /api/chat failed: 500 Internal Server Error"
        ));
        assert!(is_native_tool_transport_failure(
            "Ollama /api/chat failed: 502 Bad Gateway"
        ));
        assert!(!is_native_tool_transport_failure(
            "Ollama /api/chat failed: 429 Too Many Requests"
        ));
    }

    /// AC4 (Issue #450): when the native tool parser detects a malformed
    /// tool call, the pipeline classifies the resulting error as
    /// `FeedbackKind::ToolProtocolFailure`. We model the classification
    /// directly here because the parser-failure detector is already a
    /// `pub(super)` boolean helper — the FeedbackFrame mapping in turn.rs
    /// just calls it.
    #[test]
    fn parser_error_yields_tool_protocol_failure() {
        use super::{is_native_tool_parser_failure, is_tool_call_format_error};
        use crate::session::feedback::FeedbackKind;

        let parser_err = "native tool parser failed: unexpected end element";
        let format_err = "tool call parser failed: bad XML";
        let transport_err = "Ollama /api/chat failed: 500";

        // Helper closure mirroring the agent-layer classification logic.
        let classify = |err: &str| {
            if is_native_tool_parser_failure(err)
                || is_tool_call_format_error(err)
                || super::is_native_tool_transport_failure(err)
            {
                FeedbackKind::ToolProtocolFailure
            } else {
                FeedbackKind::UnknownFailure
            }
        };

        assert_eq!(classify(parser_err), FeedbackKind::ToolProtocolFailure);
        assert_eq!(classify(format_err), FeedbackKind::ToolProtocolFailure);
        assert_eq!(classify(transport_err), FeedbackKind::ToolProtocolFailure);
        assert_eq!(classify("nothing weird here"), FeedbackKind::UnknownFailure);
    }

    #[test]
    fn japanese_stage_three_headings_are_normalized() {
        let contents = "# Plan

## Goal
- Update src/app/page.tsx.

## Constraints
- b

## Deliverables
- c

## Acceptance Criteria
- d

## Quality Bar
- Anchor polish to `src/app/page.tsx`.

## 実行計画
- f

## 検証計画
- g

## リスク/フォールバック
- h
";
        assert!(plan_is_substantive(contents));
        assert!(plan_is_approval_ready(contents));
        assert_eq!(current_plan_stage(contents), PlanStage::Ready);
    }
}
