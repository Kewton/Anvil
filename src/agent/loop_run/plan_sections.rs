//! Plan-section progress helpers extracted from `turn.rs` (parent #680).
//!
//! Hosts the canonical plan-section names projection
//! (`plan_sections_with_content`, `plan_section_body_for_progress`) used by
//! the actor-loop progress renderer + tool_display, plus the shared
//! `join_sections_for_progress` joiner. Private helpers
//! (`plan_section_has_content`, `normalize_plan_heading_for_progress`)
//! implement the heading normalization SSOT.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

pub(super) fn join_sections_for_progress(sections: &[&str]) -> String {
    match sections {
        [] => String::new(),
        [one] => (*one).to_string(),
        [first, second] => format!("{first} and {second}"),
        _ => {
            let mut parts = sections[..sections.len() - 1]
                .iter()
                .map(|section| (*section).to_string())
                .collect::<Vec<_>>();
            parts.push(format!("and {}", sections[sections.len() - 1]));
            parts.join(", ")
        }
    }
}

pub(super) fn plan_sections_with_content(contents: &str) -> Vec<&'static str> {
    [
        "Goal",
        "Constraints",
        "First Action",
        "Verification",
        "Deliverables",
        "Acceptance Criteria",
        "Quality Bar",
        "Execution Plan",
        "Verification Plan",
        "Risks / Fallbacks",
    ]
    .into_iter()
    .filter(|section| plan_section_has_content(contents, section))
    .collect()
}

pub(super) fn plan_section_body_for_progress<'a>(
    contents: &'a str,
    section: &str,
) -> Option<&'a str> {
    let mut start = None;
    let mut end = contents.len();
    let mut offset = 0usize;
    for line in contents.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            if start.is_some() {
                end = offset;
                break;
            }
            if normalize_plan_heading_for_progress(heading) == section {
                start = Some(offset + line.len());
            }
        }
        offset += line.len() + 1;
    }
    start.map(|idx| &contents[idx..end])
}

fn plan_section_has_content(contents: &str, target: &str) -> bool {
    let mut in_section = false;
    for line in contents.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            in_section = normalize_plan_heading_for_progress(heading) == target;
            continue;
        }
        if !in_section {
            continue;
        }
        if trimmed.is_empty()
            || trimmed == "-"
            || matches!(
                trimmed,
                "1." | "2."
                    | "3."
                    | "1. First slice:"
                    | "2. Next phases:"
                    | "3. Review checkpoint:"
            )
        {
            continue;
        }
        return true;
    }
    false
}

fn normalize_plan_heading_for_progress(heading: &str) -> &str {
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
