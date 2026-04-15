pub fn detect_language_hint(text: &str) -> &'static str {
    if text.chars().any(|ch| {
        ('\u{3040}'..='\u{30ff}').contains(&ch) || ('\u{4e00}'..='\u{9faf}').contains(&ch)
    }) {
        "Japanese"
    } else {
        "Match the user's language"
    }
}
