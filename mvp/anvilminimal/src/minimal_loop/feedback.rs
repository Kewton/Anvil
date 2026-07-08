pub fn missing_artifacts(paths: &[String]) -> String {
    let remaining = paths.join(", ");
    format!(
        "Required artifacts are still missing. remaining: {remaining}\nCreate these exact workspace-relative paths before final response:\n{}",
        paths
            .iter()
            .map(|p| format!("- {p}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

pub fn no_tool_progress() -> String {
    "You described future work but did not call a tool. Use Write/Edit/Bash or explain why no workspace change is required.".to_string()
}

pub fn empty_response() -> String {
    "The previous assistant response was empty. Continue the task by calling the appropriate tool, or provide a concise final answer if no tool is needed.".to_string()
}

pub fn empty_response_reformulated(step_instruction: &str) -> String {
    format!(
        "The previous assistant response was empty again. Continue this exact step now:\n\n{step_instruction}\n\nRespond with tool calls for any workspace inspection, edits, setup, or verification needed for this step. Do not send an empty response."
    )
}

pub fn provider_turn_timeout(timeout_secs: u64) -> String {
    format!(
        "The previous provider turn exceeded the configured wall-clock cap of {timeout_secs}s and was discarded. Continue the same step now with the required tool calls or a concise final answer. Do not repeat long deliberation."
    )
}

pub fn completion_without_write() -> String {
    "The task appears to require workspace changes, but no Write/Edit tool call has happened yet. Create or modify the required files before final response, or explain why no file change is required.".to_string()
}

pub fn missing_capability_evidence(
    missing_evidence: &[String],
    missing_capabilities: &[String],
) -> String {
    let evidence = if missing_evidence.is_empty() {
        "- none".to_string()
    } else {
        missing_evidence
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let capabilities = if missing_capabilities.is_empty() {
        "- none".to_string()
    } else {
        missing_capabilities
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let mut remedy_keys = missing_evidence.to_vec();
    for capability in missing_capabilities {
        if !remedy_keys.contains(capability) {
            remedy_keys.push(capability.clone());
        }
    }
    let remedies = capability_evidence_remedy_lines(&remedy_keys);
    let remedies = if remedies.is_empty() {
        "- add concrete route-bound implementation evidence for each missing key".to_string()
    } else {
        remedies
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "The expected files now exist, but this interactive app/game step is not complete because capability evidence is still missing.\nMissing evidence:\n{evidence}\nMissing capabilities:\n{capabilities}\nPer-key remedies:\n{remedies}\nContinue implementation with Write/Edit. Add concrete interactive behavior, state updates, challenge/progression/failure logic, and restart/recoverable state as required. Do not answer in prose until the implementation evidence exists."
    )
}

pub fn capability_evidence_unresolved_reason(keys: &[String]) -> Option<String> {
    let mut unique = Vec::new();
    for key in keys {
        let trimmed = key.trim();
        if !trimmed.is_empty() && !unique.iter().any(|item| item == trimmed) {
            unique.push(trimmed.to_string());
        }
    }
    (!unique.is_empty()).then(|| format!("capability_evidence_unresolved:{}", unique.join(",")))
}

pub fn capability_evidence_remedy_lines(keys: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for key in keys {
        let line = capability_evidence_remedy_line(key);
        if !out.contains(&line) {
            out.push(line);
        }
    }
    out
}

pub fn capability_evidence_remedy_line(key: &str) -> String {
    match key.trim() {
        "restart_or_recoverable_state_evidence" => {
            "restart_or_recoverable_state_evidence: add data-anvil-action=\"restart\" to every restart/retry/new-game affordance (game-over, victory, and in-play when present) and ensure it resets observable state; the initial primary action alone is not restart evidence".to_string()
        }
        "user_input_handler_evidence" => {
            "user_input_handler_evidence: wire keyboard, pointer, click, touch, or form handlers to route-bound state changes".to_string()
        }
        "stateful_update_evidence" => {
            "stateful_update_evidence: update visible state over time or directly from user input, and expose the updated snapshot in data-anvil-state".to_string()
        }
        "challenge_or_adversary_evidence" => {
            "challenge_or_adversary_evidence: wire a reachable challenge, obstacle, enemy, timer, or comparable adversary into state evolution".to_string()
        }
        "score_or_progression_evidence" => {
            "score_or_progression_evidence: make score, level, progress, or win/loss state change from meaningful gameplay or interaction".to_string()
        }
        "failure_or_collision_evidence" => {
            "failure_or_collision_evidence: implement a reachable failure, collision, timeout, or loss condition that changes visible state".to_string()
        }
        "interactive_ui_source_evidence" => {
            "interactive_ui_source_evidence: keep the interactive implementation route-bound from the page entrypoint, not stranded in an unimported component".to_string()
        }
        "visible_interactive_surface_evidence" => {
            "visible_interactive_surface_evidence: render a visible interactive surface such as controls, canvas, board, form, or active play area".to_string()
        }
        "non_static_screen_evidence" => {
            "non_static_screen_evidence: make the rendered screen change from state, input, timer, or progression instead of staying static".to_string()
        }
        "implementation_artifact" => {
            "implementation_artifact: create route-bound task implementation files rather than only plans, notes, or scaffold".to_string()
        }
        other if other.starts_with("required_obligation:") => {
            format!("{other}: satisfy the named required obligation in the route-bound implementation")
        }
        "" => {
            "capability_evidence: add route-bound implementation evidence for the pending contract key".to_string()
        }
        other => {
            format!("{other}: add concrete route-bound implementation evidence for this pending contract key")
        }
    }
}

pub fn malformed_tool_call(error: &str) -> String {
    format!("The previous tool call was malformed: {error}. Retry with a valid tool call.")
}

pub fn artifact_stagnation(paths: &[String], attempt: usize, attempt_limit: usize) -> String {
    format!(
        "Required artifact creation is stalled. Missing required artifact(s): {}.\nEmit exactly one Write or Edit tool call now for one of those paths. Do not inspect the workspace again and do not answer in prose until a required artifact is created. artifact_recovery_attempt={attempt}/{attempt_limit}",
        paths.join(", ")
    )
}

pub fn artifact_stagnation_for_target(
    paths: &[String],
    target_path: &str,
    attempt: usize,
    attempt_limit: usize,
) -> String {
    if target_path.is_empty() {
        return artifact_stagnation(paths, attempt, attempt_limit);
    }
    format!(
        "Required artifact creation is stalled. Missing required artifact(s): {}.\nCreate this exact workspace-relative path now: `{target_path}`.\nYour next response for this turn must be exactly one Write or Edit tool call for `{target_path}`. Plain text without a tool call is invalid and will be discarded. Do not inspect the workspace again and do not answer in prose until this required artifact is created. artifact_recovery_target_attempt={attempt}/{attempt_limit}",
        paths.join(", ")
    )
}

pub fn read_only_stagnation(objective: &str, streak: usize) -> String {
    format!(
        "Inspection is sufficient - implement now via Write/Edit; remaining objective: {objective}. Your next response must mutate the workspace with exactly one Write or Edit tool call. read_only_streak={streak}"
    )
}

pub fn read_only_stagnation_compact(objective: &str, streak: usize) -> String {
    format!(
        "Compact restatement: implement the requested change now. Objective: {objective}. Do not inspect again, do not answer in prose, and do not run verification. Use Write or Edit for the concrete implementation change. read_only_streak={streak}"
    )
}

pub fn verify_repair_edit_required(
    signature: &str,
    attempt: usize,
    attempt_limit: usize,
) -> String {
    format!(
        "Deterministic verification is still failing with the same signature: {signature}. Do not rerun verification and do not answer in prose. Make a concrete Write or Edit change to the failing implementation, test, or setup file before verification is retried. verify_repair_edit_attempt={attempt}/{attempt_limit}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_artifacts_feedback_names_remaining_paths_inline() {
        let feedback = missing_artifacts(&[
            "postcss.config.js".to_string(),
            "tailwind.config.ts".to_string(),
        ]);

        assert!(
            feedback.contains("remaining: postcss.config.js, tailwind.config.ts"),
            "{feedback}"
        );
        assert!(feedback.contains("- postcss.config.js"), "{feedback}");
        assert!(feedback.contains("- tailwind.config.ts"), "{feedback}");
    }

    #[test]
    fn missing_capability_feedback_lists_per_key_remedies() {
        let feedback = missing_capability_evidence(
            &["restart_or_recoverable_state_evidence".to_string()],
            &[],
        );

        assert!(feedback.contains("Per-key remedies:"), "{feedback}");
        assert!(
            feedback.contains("data-anvil-action=\"restart\""),
            "{feedback}"
        );
        assert!(
            feedback.contains("initial primary action alone is not restart evidence"),
            "{feedback}"
        );
    }

    #[test]
    fn unresolved_reason_names_pending_keys() {
        let reason = capability_evidence_unresolved_reason(&[
            "restart_or_recoverable_state_evidence".to_string(),
            "restart_or_recoverable_state_evidence".to_string(),
        ])
        .unwrap();

        assert_eq!(
            reason,
            "capability_evidence_unresolved:restart_or_recoverable_state_evidence"
        );
    }
}
