//! Request-text signal extraction for task-contract interpretation.
//!
//! This module is deliberately lexical. It extracts small, generic signals
//! from already-normalized request text, but it does not decide task kind,
//! deliverables, evidence, or terminal state.

pub(super) fn negated_artifact_list_contains(lower: &str, artifact_tokens: &[&str]) -> bool {
    const PREFIXES: &[&str] = &[
        "do not create",
        "do not write",
        "do not add",
        "do not implement",
        "don't create",
        "don't write",
        "don't add",
        "don't implement",
    ];

    PREFIXES.iter().any(|prefix| {
        lower.match_indices(prefix).any(|(idx, _)| {
            let clause = negated_artifact_clause(&lower[idx + prefix.len()..]);
            artifact_tokens
                .iter()
                .any(|token| contains_ascii_token(clause, token))
        })
    })
}

pub(super) fn contains_implementation_file_hint(lower: &str) -> bool {
    [
        ".rs", ".py", ".ts", ".tsx", ".js", ".jsx", ".vue", ".svelte", ".go", ".java", ".kt",
        ".swift",
    ]
    .iter()
    .any(|suffix| lower_contains_file_suffix(lower, suffix))
}

pub(super) fn contains_callable_signature_hint(lower: &str) -> bool {
    lower.contains('(')
        && lower.contains(')')
        && (lower.contains("->") || contains_dotted_callable_hint(lower))
}

pub(super) fn contains_dotted_callable_change_action(lower: &str) -> bool {
    dotted_callable_starts(lower).into_iter().any(|start| {
        last_ascii_word(&lower[..start]).is_some_and(|word| {
            matches!(
                word,
                "add" | "implement" | "create" | "write" | "update" | "modify"
            )
        })
    })
}

fn negated_artifact_clause(after_prefix: &str) -> &str {
    let sentence_end = after_prefix
        .char_indices()
        .find_map(|(idx, ch)| matches!(ch, '.' | '\n' | '\r' | ';').then_some(idx))
        .unwrap_or(after_prefix.len());
    let sentence = &after_prefix[..sentence_end];
    sentence
        .split(" but ")
        .next()
        .unwrap_or(sentence)
        .split(" however ")
        .next()
        .unwrap_or(sentence)
}

fn contains_dotted_callable_hint(lower: &str) -> bool {
    !dotted_callable_starts(lower).is_empty()
}

fn dotted_callable_starts(lower: &str) -> Vec<usize> {
    lower
        .match_indices('(')
        .filter_map(|(paren_idx, _)| {
            lower[paren_idx..].contains(')').then_some(())?;
            dotted_identifier_start_before_paren(&lower[..paren_idx])
        })
        .collect()
}

fn dotted_identifier_start_before_paren(before_paren: &str) -> Option<usize> {
    let trimmed = before_paren.trim_end();
    if !trimmed
        .chars()
        .last()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        return None;
    }
    let mut start = trimmed.len();
    for (idx, ch) in trimmed.char_indices().rev() {
        if ch == '.' || ch == '_' || ch.is_ascii_alphanumeric() {
            start = idx;
        } else {
            break;
        }
    }
    let token = &trimmed[start..];
    let valid = token.contains('.')
        && token.split('.').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        });
    valid.then_some(start)
}

fn last_ascii_word(prefix: &str) -> Option<&str> {
    prefix
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .rfind(|word| !word.is_empty())
}

fn lower_contains_file_suffix(lower: &str, suffix: &str) -> bool {
    lower.match_indices(suffix).any(|(idx, _)| {
        let after_idx = idx + suffix.len();
        lower[after_idx..]
            .chars()
            .next()
            .is_none_or(|ch| !ch.is_ascii_alphanumeric())
    })
}

fn contains_ascii_token(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(idx, _)| {
        let before_ok = idx == 0
            || haystack[..idx]
                .chars()
                .next_back()
                .is_none_or(|ch| !ch.is_ascii_alphanumeric());
        let after_idx = idx + needle.len();
        let after_ok = after_idx >= haystack.len()
            || haystack[after_idx..]
                .chars()
                .next()
                .is_none_or(|ch| !ch.is_ascii_alphanumeric());
        before_ok && after_ok
    })
}
