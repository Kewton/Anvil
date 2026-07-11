use std::path::Path;

use serde_json::json;

use crate::eval_events;
use crate::planner::verify::VerificationReport;

pub const CONTRACT_ATTRIBUTE_MISSING_KIND: &str = "contract_attribute_missing";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractAttributeIssue {
    pub attribute: String,
    pub path: String,
}

pub fn detect(report: &VerificationReport) -> Option<ContractAttributeIssue> {
    report
        .command_failures
        .iter()
        .find_map(|failure| issue_from_text(&failure.command, &failure.reason))
        .or_else(|| {
            report
                .profile_failures
                .iter()
                .find_map(|failure| issue_from_text(failure, failure))
        })
}

pub fn is_contract_attribute_missing(report: &VerificationReport) -> bool {
    detect(report).is_some()
}

pub fn merge_repair_target_paths(report: &VerificationReport, paths: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(issue) = detect(report) {
        push_unique(&mut out, issue.path);
    }
    for path in paths {
        push_unique(&mut out, path.clone());
    }
    out
}

pub fn guidance_section(
    root: Option<&Path>,
    report: &VerificationReport,
    eval_events_path: Option<&Path>,
) -> String {
    let Some(issue) = detect(report) else {
        return String::new();
    };
    guidance_for_issue(root, &issue, eval_events_path)
}

pub(crate) fn guidance_for_issue(
    root: Option<&Path>,
    issue: &ContractAttributeIssue,
    eval_events_path: Option<&Path>,
) -> String {
    emit_guidance_event(eval_events_path, &issue);
    format!(
        "Contract attribute repair guidance:\n\
- classification: {CONTRACT_ATTRIBUTE_MISSING_KIND}\n\
- missing attribute: `{attribute}`\n\
- target source file: `{path}`\n\
- contract requirement: {requirement}\n\
- location directive: add the missing attribute to the route-bound container, control, or status element in the same rendered hierarchy as the existing hooks below.\n\
Existing hook locations:\n{excerpts}\n\
Generic one-line example:\n- {example}\n",
        attribute = issue.attribute,
        path = issue.path,
        requirement = requirement_for_attribute(&issue.attribute),
        excerpts = hook_location_excerpts(root, &issue.path),
        example = example_for_attribute(&issue.attribute),
    )
}

fn emit_guidance_event(eval_events_path: Option<&Path>, issue: &ContractAttributeIssue) {
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "contract_attribute_repair_guidance",
            "attribute": issue.attribute,
            "path": issue.path,
        }),
    );
}

fn issue_from_text(command: &str, reason: &str) -> Option<ContractAttributeIssue> {
    let text = format!("{command}\n{reason}");
    let attribute = missing_attribute_name(&text)?;
    let path = source_path_from_text(&text)?;
    Some(ContractAttributeIssue { attribute, path })
}

fn missing_attribute_name(text: &str) -> Option<String> {
    if text.contains("data-anvil-state") {
        return Some("data-anvil-state".to_string());
    }
    if text.contains("data-anvil-action") {
        return Some(action_attribute_name(text));
    }
    None
}

fn action_attribute_name(text: &str) -> String {
    for value in ["primary", "restart", "input", "search", "submit"] {
        if text.contains(value) {
            return format!("data-anvil-action=\"{value}\"");
        }
    }
    "data-anvil-action".to_string()
}

fn source_path_from_text(text: &str) -> Option<String> {
    extract_between(text, "readFileSync(\"", "\"")
        .or_else(|| extract_between(text, "readFileSync('", "'"))
        .or_else(|| extract_between(text, "readFileSync(\\\"", "\\\""))
        .or_else(|| source_path_token(text))
        .map(|path| path.trim_start_matches("./").replace('\\', "/"))
        .filter(|path| looks_like_source_path(path))
}

fn extract_between(text: &str, prefix: &str, suffix: &str) -> Option<String> {
    let start = text.find(prefix)? + prefix.len();
    let rest = &text[start..];
    let end = rest.find(suffix)?;
    Some(rest[..end].to_string())
}

fn source_path_token(text: &str) -> Option<String> {
    text.split(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | '`' | ',' | ')' | '('))
        .map(|token| {
            token
                .trim()
                .trim_start_matches("./")
                .trim_matches(|ch: char| matches!(ch, ':' | ';' | '[' | ']'))
        })
        .find(|token| looks_like_source_path(token))
        .map(str::to_string)
}

fn looks_like_source_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.contains("..") {
        return false;
    }
    matches!(
        Path::new(path).extension().and_then(|ext| ext.to_str()),
        Some("tsx" | "ts" | "jsx" | "js")
    )
}

fn requirement_for_attribute(attribute: &str) -> &'static str {
    if attribute == "data-anvil-state" {
        "provide a meaningful visible-state JSON snapshot after each render, and include at least one input-coupled dimension such as a player or paddle x coordinate."
    } else if attribute.contains("restart") {
        "mark every restart, retry, or new-game affordance that resets observable state; the primary action alone is not restart evidence."
    } else if attribute.contains("input") {
        "mark the main text entry surface when one exists so interaction probes can target input behavior."
    } else {
        "mark the main start, submit, or primary action control so interaction probes can start the experience deterministically."
    }
}

fn example_for_attribute(attribute: &str) -> &'static str {
    if attribute == "data-anvil-state" {
        r#"data-anvil-state={JSON.stringify({ phase, score, playerX })}"#
    } else if attribute.contains("restart") {
        r#"data-anvil-action="restart""#
    } else if attribute.contains("input") {
        r#"data-anvil-action="input""#
    } else {
        r#"data-anvil-action="primary""#
    }
}

fn hook_location_excerpts(root: Option<&Path>, path: &str) -> String {
    let Some(root) = root else {
        return "- source excerpt unavailable".to_string();
    };
    let Ok(source) = std::fs::read_to_string(root.join(path)) else {
        return "- source excerpt unavailable".to_string();
    };
    let lines = source.lines().collect::<Vec<_>>();
    let hook_indexes = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            line.contains("data-anvil-action")
                .then_some(index)
                .or_else(|| line.contains("data-anvil-state").then_some(index))
        })
        .take(3)
        .collect::<Vec<_>>();
    if hook_indexes.is_empty() {
        return jsx_location_excerpt(&lines).unwrap_or_else(|| {
            "- no existing data-anvil hook lines found in the target file".to_string()
        });
    }
    hook_indexes
        .into_iter()
        .map(|index| line_numbered_excerpt(&lines, index))
        .collect::<Vec<_>>()
        .join("\n")
}

fn jsx_location_excerpt(lines: &[&str]) -> Option<String> {
    let index = lines.iter().position(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("return <")
            || trimmed.starts_with("return (")
            || trimmed.starts_with("<main")
            || trimmed.starts_with("<section")
            || trimmed.starts_with("<div")
            || trimmed.contains("return <main")
            || trimmed.contains("return <section")
            || trimmed.contains("return <div")
    })?;
    Some(format!(
        "- no existing data-anvil hook lines found; likely insertion area\n{}",
        line_numbered_excerpt(lines, index)
    ))
}

fn line_numbered_excerpt(lines: &[&str], index: usize) -> String {
    let start = index.saturating_sub(2);
    let end = (index + 3).min(lines.len());
    let mut out = format!("- near line {}\n", index + 1);
    for line_index in start..end {
        out.push_str(&format!(
            "  {:>4} | {}\n",
            line_index + 1,
            lines[line_index]
        ));
    }
    out.trim_end().to_string()
}

fn push_unique(out: &mut Vec<String>, value: String) {
    if !value.trim().is_empty() && !out.iter().any(|existing| existing == &value) {
        out.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::verify::VerificationReport;
    use serde_json::Value;

    fn report_with_failure(command: &str, reason: &str) -> VerificationReport {
        let mut report = VerificationReport::pass();
        report.push_command_failure(command, reason);
        report
    }

    fn event_values(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect()
    }

    #[test]
    fn detects_state_attribute_failure_and_renders_guidance_with_hook_excerpts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            [
                "export default function Page() {",
                "  return (",
                "    <main>",
                "      <button data-anvil-action=\"primary\">Start</button>",
                "      <section>Status</section>",
                "      <button data-anvil-action=\"restart\">Restart</button>",
                "    </main>",
                "  );",
                "}",
                "",
            ]
            .join("\n"),
        )
        .unwrap();
        let events = dir.path().join("events.jsonl");
        let report = report_with_failure(
            r#"node -p '(function(s,w,b){return /data-anvil-state/.test(s)?true:process.exit(1)})(String(require("fs").readFileSync("src/app/page.tsx")))' "#,
            "command failed",
        );

        let issue = detect(&report).unwrap();
        assert_eq!(issue.attribute, "data-anvil-state");
        assert_eq!(issue.path, "src/app/page.tsx");

        let guidance = guidance_section(Some(dir.path()), &report, Some(&events));
        assert!(
            guidance.contains(CONTRACT_ATTRIBUTE_MISSING_KIND),
            "{guidance}"
        );
        assert!(guidance.contains("data-anvil-state"), "{guidance}");
        assert!(
            guidance.contains("meaningful visible-state JSON snapshot"),
            "{guidance}"
        );
        assert!(guidance.contains("input-coupled dimension"), "{guidance}");
        assert!(guidance.contains("line 4"), "{guidance}");
        assert!(guidance.contains("line 6"), "{guidance}");
        assert!(
            guidance.contains(r#"data-anvil-state={JSON.stringify({ phase, score, playerX })}"#),
            "{guidance}"
        );
        let events = event_values(&events);
        assert_eq!(events[0]["event"], "contract_attribute_repair_guidance");
        assert_eq!(events[0]["attribute"], "data-anvil-state");
        assert_eq!(events[0]["path"], "src/app/page.tsx");
    }

    #[test]
    fn detects_action_attribute_failure() {
        let report = report_with_failure(
            r#"node -p '(function(s,w,d){return /data-anvil-action/.test(s)?true:process.exit(1)})(String(require("fs").readFileSync("src/app/page.tsx")),"primary")'"#,
            "command failed",
        );

        let issue = detect(&report).unwrap();

        assert_eq!(issue.attribute, "data-anvil-action=\"primary\"");
        assert_eq!(issue.path, "src/app/page.tsx");
        let guidance = guidance_section(None, &report, None);
        assert!(guidance.contains("main start, submit, or primary action control"));
        assert!(guidance.contains(r#"data-anvil-action="primary""#));
    }

    #[test]
    fn non_contract_verify_failure_is_ignored() {
        let report = report_with_failure("npm run build", "Type error: Cannot find name 'x'");

        assert!(detect(&report).is_none());
        assert!(!is_contract_attribute_missing(&report));
        assert!(guidance_section(None, &report, None).is_empty());
    }

    #[test]
    fn repair_target_paths_prepend_contract_source() {
        let report = report_with_failure(
            r#"node -p 'String(require("fs").readFileSync("src/app/page.tsx")).includes("data-anvil-state") ? true : process.exit(1)'"#,
            "command failed",
        );

        assert_eq!(
            merge_repair_target_paths(&report, &["package.json".to_string()]),
            vec!["src/app/page.tsx", "package.json"]
        );
    }
}
