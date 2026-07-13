use std::path::Path;

use serde_json::Value;

use crate::planner::profile::{
    InteractionRepairContract, interaction_repair_contract, profile_interaction_repair_guidance,
};

pub(crate) fn inferred_guidance(
    profile: &str,
    goal: &str,
    failure_kind: &str,
    evidence: Option<&Value>,
) -> Vec<String> {
    let contract = interaction_repair_contract(profile, goal);
    guidance(profile, failure_kind, &contract, evidence)
}

pub(crate) fn guidance(
    profile: &str,
    failure_kind: &str,
    contract: &InteractionRepairContract,
    evidence: Option<&Value>,
) -> Vec<String> {
    let mut lines = if render_loop_failure(failure_kind) {
        evidence
            .map(unattached_ref_guidance_lines)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    merge_unique(
        &mut lines,
        &profile_interaction_repair_guidance(profile, failure_kind, contract),
    );
    lines
}

fn render_loop_failure(failure_kind: &str) -> bool {
    let lower = failure_kind.to_ascii_lowercase();
    lower.contains("input_state_change_missing_after_start") || lower.contains("canvas_blank")
}

fn unattached_ref_guidance_lines(value: &Value) -> Vec<String> {
    value_scopes(value)
        .into_iter()
        .find_map(|scope| {
            scope
                .get("unattached_ref_diagnostics")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(unattached_ref_guidance_line)
                        .collect::<Vec<_>>()
                })
                .filter(|items| !items.is_empty())
        })
        .unwrap_or_default()
}

fn unattached_ref_guidance_line(value: &Value) -> Option<String> {
    if let Some(guidance) = value
        .get("guidance")
        .and_then(Value::as_str)
        .filter(|guidance| !guidance.trim().is_empty())
    {
        return Some(guidance.trim().to_string());
    }
    let name = value.get("name").and_then(Value::as_str)?;
    let candidate = value
        .get("candidate_elements")
        .and_then(Value::as_array)?
        .first()?;
    let tag = candidate.get("tag").and_then(Value::as_str)?;
    let source = candidate.get("source").and_then(Value::as_str)?;
    let line = candidate.get("line").and_then(Value::as_u64)?;
    let file = Path::new(source)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(source);
    Some(format!(
        "attach ref={{{name}}} to the <{tag}> at {file}:{line}"
    ))
}

fn value_scopes(value: &Value) -> Vec<&Value> {
    let mut scopes = vec![value];
    for key in ["details", "browser_details"] {
        if let Some(details) = value.get(key).filter(|details| details.is_object()) {
            scopes.push(details);
        }
    }
    scopes
}

fn merge_unique(out: &mut Vec<String>, incoming: &[String]) {
    for value in incoming {
        if !out.iter().any(|existing| existing == value) {
            out.push(value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiz_contract_keeps_unattached_diagnostic_without_canvas_checklists() {
        let contract = InteractionRepairContract {
            required_capabilities: vec!["stateful_interaction".to_string()],
            required_evidence: vec!["stateful_update_evidence".to_string()],
        };
        let evidence = serde_json::json!({
            "unattached_ref_diagnostics": [{
                "guidance": "attach ref={answerRef} to the <button> at page.tsx:44"
            }]
        });

        let guidance = guidance(
            "nextjs",
            "browser_interaction_failed:input_state_change_missing_after_start",
            &contract,
            Some(&evidence),
        );

        assert_eq!(
            guidance.first().map(String::as_str),
            Some("attach ref={answerRef} to the <button> at page.tsx:44")
        );
        assert!(guidance.iter().all(|line| !line.contains("projectiles")));
        assert!(guidance.iter().all(|line| !line.contains("rAF loop")));
    }
}
