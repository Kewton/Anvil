pub fn missing_artifacts(paths: &[String]) -> String {
    format!(
        "Required artifacts are still missing. Create these exact workspace-relative paths before final response:\n{}",
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

pub fn completion_without_write() -> String {
    "The task appears to require workspace changes, but no Write/Edit tool call has happened yet. Create or modify the required files before final response, or explain why no file change is required.".to_string()
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

pub fn verify_repair_edit_required(
    signature: &str,
    attempt: usize,
    attempt_limit: usize,
) -> String {
    format!(
        "Deterministic verification is still failing with the same signature: {signature}. Do not rerun verification and do not answer in prose. Make a concrete Write or Edit change to the failing implementation, test, or setup file before verification is retried. verify_repair_edit_attempt={attempt}/{attempt_limit}"
    )
}
