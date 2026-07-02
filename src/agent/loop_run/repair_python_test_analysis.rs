use std::collections::HashSet;

pub(crate) fn excerpt_has_disconnected_fixture_state_assertion(excerpt: &str) -> bool {
    let fixture_names = pytest_local_mutable_fixture_names(excerpt);
    if fixture_names.is_empty() {
        return false;
    }
    fixture_names.iter().any(|name| {
        excerpt_asserts_fixture_state(excerpt, name)
            && !excerpt_connects_fixture_to_system_under_test(excerpt, name)
    })
}

pub(crate) fn pytest_local_mutable_fixture_names(excerpt: &str) -> HashSet<String> {
    let lines = excerpt.lines().collect::<Vec<_>>();
    let mut fixtures = HashSet::new();
    let mut index = 0usize;
    while index < lines.len() {
        if !lines[index].trim_start().starts_with("@pytest.fixture") {
            index += 1;
            continue;
        }
        let mut def_index = index + 1;
        while def_index < lines.len() && lines[def_index].trim().is_empty() {
            def_index += 1;
        }
        let Some(name) = lines
            .get(def_index)
            .and_then(|line| python_def_name(line.trim_start()))
        else {
            index += 1;
            continue;
        };
        let body_indent = lines
            .get(def_index)
            .map(|line| line.chars().take_while(|ch| ch.is_whitespace()).count() + 1)
            .unwrap_or(1);
        let mut body = String::new();
        let mut body_index = def_index + 1;
        while body_index < lines.len() {
            let line = lines[body_index];
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let indent = line.chars().take_while(|ch| ch.is_whitespace()).count();
                if indent < body_indent {
                    break;
                }
            }
            body.push_str(line);
            body.push('\n');
            body_index += 1;
        }
        if fixture_body_returns_local_mutable(&body) {
            fixtures.insert(name.to_string());
        }
        index = body_index.max(index + 1);
    }
    fixtures
}

pub(crate) fn excerpt_asserts_fixture_state(excerpt: &str, fixture_name: &str) -> bool {
    excerpt.lines().any(|line| {
        let stripped = strip_inline_comment(line);
        let trimmed = stripped.trim_start();
        trimmed.starts_with("assert ")
            && (trimmed.contains(&format!("len({fixture_name})"))
                || trimmed.contains(&format!("{fixture_name} =="))
                || trimmed.contains(&format!("{fixture_name} !="))
                || trimmed.contains(&format!("{fixture_name}[")))
    })
}

pub(crate) fn excerpt_connects_fixture_to_system_under_test(
    excerpt: &str,
    fixture_name: &str,
) -> bool {
    excerpt.lines().any(|line| {
        let stripped = strip_inline_comment(line);
        let trimmed = stripped.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("assert ")
            || trimmed.starts_with("def ")
            || trimmed.starts_with("@")
            || trimmed.starts_with("return ")
        {
            return false;
        }
        if !line_mentions_identifier(trimmed, fixture_name) {
            return false;
        }
        trimmed.contains("dependency_overrides")
            || trimmed.contains("monkeypatch")
            || trimmed.contains("setattr(")
            || trimmed.contains(".state.")
            || trimmed.contains(".app.")
            || trimmed.contains("override")
            || trimmed.contains("patch(")
    })
}

pub(crate) fn line_mentions_identifier(line: &str, name: &str) -> bool {
    if !identifier_is_safe(name) {
        return false;
    }
    line.match_indices(name).any(|(idx, _)| {
        let before_ok = line[..idx]
            .chars()
            .next_back()
            .is_none_or(|ch| !identifier_char(ch));
        let after_index = idx + name.len();
        let after_ok = line[after_index..]
            .chars()
            .next()
            .is_none_or(|ch| !identifier_char(ch));
        before_ok && after_ok
    })
}

fn python_def_name(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("def ")?;
    let name = rest.split_once('(')?.0.trim();
    identifier_is_safe(name).then_some(name)
}

fn fixture_body_returns_local_mutable(body: &str) -> bool {
    body.lines().any(|line| {
        let stripped = strip_inline_comment(line);
        let trimmed = stripped.trim();
        matches!(
            trimmed,
            "return {}" | "return []" | "return set()" | "return dict()" | "return list()"
        )
    })
}

fn strip_inline_comment(line: &str) -> String {
    line.split_once('#')
        .map(|(head, _)| head)
        .unwrap_or(line)
        .to_string()
}

fn identifier_is_safe(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_fixture_state_assertion_detects_local_state_observation() {
        let excerpt = r#"
@pytest.fixture
def store():
    return {}

def test_list(store):
    store["item"] = 1
    assert len(store) == 1
"#;

        assert!(excerpt_has_disconnected_fixture_state_assertion(excerpt));
    }

    #[test]
    fn connected_fixture_state_assertion_is_not_disconnected() {
        let excerpt = r#"
@pytest.fixture
def store():
    return {}

def test_list(store, monkeypatch):
    monkeypatch.setattr(app, "state", store)
    assert len(store) == 1
"#;

        assert!(!excerpt_has_disconnected_fixture_state_assertion(excerpt));
    }

    #[test]
    fn line_identifier_match_respects_boundaries() {
        assert!(line_mentions_identifier("assert user_id == 1", "user_id"));
        assert!(!line_mentions_identifier(
            "assert other_user_id == 1",
            "user_id"
        ));
    }
}
