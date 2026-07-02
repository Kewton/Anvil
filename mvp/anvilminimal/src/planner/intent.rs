pub fn detect_intent(goal: &str) -> &'static str {
    let lower = goal.to_ascii_lowercase();
    if lower.contains("fix") || lower.contains("修正") {
        "fix"
    } else if lower.contains("research") || lower.contains("調査") {
        "research"
    } else {
        "create"
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn create_fix_research_intent() {
        assert_eq!(super::detect_intent("fix parser"), "fix");
        assert_eq!(super::detect_intent("research topic"), "research");
        assert_eq!(super::detect_intent("make app"), "create");
    }
}
