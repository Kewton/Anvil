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
