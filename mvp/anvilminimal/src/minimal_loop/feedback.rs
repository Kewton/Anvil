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

pub fn malformed_tool_call(error: &str) -> String {
    format!("The previous tool call was malformed: {error}. Retry with a valid tool call.")
}
