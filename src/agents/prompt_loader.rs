pub fn prompt_path(role: &str) -> Option<&'static str> {
    match role {
        "pm" => Some("prompts/pm.txt"),
        "reader" => Some("prompts/reader.txt"),
        "editor" => Some("prompts/editor.txt"),
        "tester" => Some("prompts/tester.txt"),
        "reviewer" => Some("prompts/reviewer.txt"),
        _ => None,
    }
}
