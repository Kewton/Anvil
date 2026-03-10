pub fn template_name_for_role(role: &str) -> Option<&'static str> {
    match role {
        "pm" => Some("pm"),
        "reader" => Some("reader"),
        "editor" => Some("editor"),
        "tester" => Some("tester"),
        "reviewer" => Some("reviewer"),
        _ => None,
    }
}
