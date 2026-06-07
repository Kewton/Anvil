use std::path::Path;

pub(super) fn self_referential_node_verifier_command(
    work_root: &Path,
    relative_path: &str,
    source: &str,
) -> Option<String> {
    if !is_javascript_test_artifact(relative_path) {
        return None;
    }
    let package_test_script = package_manifest_test_script(work_root);
    javascript_child_process_commands(source)
        .into_iter()
        .find(|command| {
            node_verifier_command_is_self_reference(command, package_test_script.as_deref())
        })
}

fn is_javascript_test_artifact(relative_path: &str) -> bool {
    matches!(
        Path::new(relative_path)
            .extension()
            .and_then(|ext| ext.to_str()),
        Some("js" | "mjs" | "cjs" | "ts" | "tsx")
    )
}

fn package_manifest_test_script(work_root: &Path) -> Option<String> {
    let source = std::fs::read_to_string(work_root.join("package.json")).ok()?;
    let summary = super::package_manifest_summary::parse_package_manifest_summary(&source).ok()?;
    summary.scripts.get("test").cloned()
}

fn node_verifier_command_is_self_reference(
    command: &str,
    package_test_script: Option<&str>,
) -> bool {
    let command = normalize_child_process_command(command);
    if command.is_empty() {
        return false;
    }
    if command_starts_with_tokens(&command, &["npm", "test"])
        || command_starts_with_tokens(&command, &["npm", "run", "test"])
        || command_starts_with_tokens(&command, &["node", "--test"])
    {
        return true;
    }
    let Some(script) = package_test_script else {
        return false;
    };
    let script = normalize_child_process_command(script);
    !script.is_empty() && command == script
}

fn command_starts_with_tokens(command: &str, prefix: &[&str]) -> bool {
    let tokens = command.split_whitespace().collect::<Vec<_>>();
    tokens.len() >= prefix.len()
        && tokens
            .iter()
            .zip(prefix.iter())
            .all(|(token, expected)| token.eq_ignore_ascii_case(expected))
}

fn normalize_child_process_command(command: &str) -> String {
    command
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn javascript_child_process_commands(source: &str) -> Vec<String> {
    let uncommented = strip_javascript_comments(source);
    let mut commands = Vec::new();
    for marker in ["execSync(", "exec("] {
        commands.extend(first_js_call_string_literals(&uncommented, marker));
    }
    for marker in ["spawnSync(", "spawn(", "execFileSync(", "execFile("] {
        commands.extend(js_spawn_like_commands(&uncommented, marker));
    }
    commands
}

fn first_js_call_string_literals(source: &str, marker: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut tail = source;
    while let Some(idx) = tail.find(marker) {
        let after = &tail[idx + marker.len()..];
        if let Some((literal, consumed)) = first_js_string_literal(after) {
            out.push(literal);
            tail = &after[consumed..];
        } else {
            tail = after;
        }
    }
    out
}

fn js_spawn_like_commands(source: &str, marker: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut tail = source;
    while let Some(idx) = tail.find(marker) {
        let after = &tail[idx + marker.len()..];
        let Some((program, consumed)) = first_js_string_literal(after) else {
            tail = after;
            continue;
        };
        let rest = &after[consumed..after.find(')').unwrap_or(after.len())];
        let mut tokens = vec![program];
        if let Some(args) = first_js_string_array_literals(rest) {
            tokens.extend(args);
        }
        out.push(tokens.join(" "));
        tail = &after[consumed..];
    }
    out
}

fn first_js_string_array_literals(source: &str) -> Option<Vec<String>> {
    let start = source.find('[')?;
    let end = source[start..].find(']')? + start;
    Some(all_js_string_literals(&source[start + 1..end]))
}

fn first_js_string_literal(source: &str) -> Option<(String, usize)> {
    let bytes = source.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        let quote = bytes[idx] as char;
        if matches!(quote, '"' | '\'' | '`') {
            let (literal, end) = read_js_string_literal(source, idx, quote)?;
            return Some((literal, end));
        }
        idx += 1;
    }
    None
}

fn all_js_string_literals(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut idx = 0;
    while idx < source.len() {
        let quote = source.as_bytes()[idx] as char;
        if matches!(quote, '"' | '\'' | '`')
            && let Some((literal, end)) = read_js_string_literal(source, idx, quote)
        {
            out.push(literal);
            idx = end;
            continue;
        }
        idx += 1;
    }
    out
}

fn read_js_string_literal(source: &str, start: usize, quote: char) -> Option<(String, usize)> {
    let mut escaped = false;
    let mut out = String::new();
    for (offset, ch) in source[start + quote.len_utf8()..].char_indices() {
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            return Some((out, start + quote.len_utf8() + offset + quote.len_utf8()));
        }
        out.push(ch);
    }
    None
}

fn strip_javascript_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut string_quote: Option<char> = None;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if let Some(quote) = string_quote {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                string_quote = None;
            }
            continue;
        }
        if matches!(ch, '"' | '\'' | '`') {
            string_quote = Some(ch);
            out.push(ch);
            continue;
        }
        if ch == '/' && chars.peek().is_some_and(|next| *next == '/') {
            chars.next();
            for comment_ch in chars.by_ref() {
                if comment_ch == '\n' {
                    out.push('\n');
                    break;
                }
            }
            continue;
        }
        if ch == '/' && chars.peek().is_some_and(|next| *next == '*') {
            chars.next();
            let mut prev = '\0';
            for comment_ch in chars.by_ref() {
                if comment_ch == '\n' {
                    out.push('\n');
                }
                if prev == '*' && comment_ch == '/' {
                    break;
                }
                prev = comment_ch;
            }
            continue;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_package(root: &Path, contents: &str) {
        std::fs::write(root.join("package.json"), contents).expect("write package");
    }

    #[test]
    fn detects_execsync_npm_test_recursion() {
        let root = tempfile::tempdir().expect("tempdir");
        write_package(root.path(), r#"{"scripts":{"test":"node --test"}}"#);
        let command = self_referential_node_verifier_command(
            root.path(),
            "tests/index.test.js",
            "execSync('npm test', { stdio: 'inherit' });",
        );

        assert_eq!(command.as_deref(), Some("npm test"));
    }

    #[test]
    fn detects_spawn_node_test_recursion() {
        let root = tempfile::tempdir().expect("tempdir");
        write_package(root.path(), r#"{"scripts":{"test":"node --test"}}"#);
        let command = self_referential_node_verifier_command(
            root.path(),
            "tests/index.test.js",
            "spawnSync('node', ['--test', 'tests/index.test.js']);",
        );

        assert_eq!(command.as_deref(), Some("node --test tests/index.test.js"));
    }

    #[test]
    fn allows_sut_cli_child_process_invocation() {
        let root = tempfile::tempdir().expect("tempdir");
        write_package(root.path(), r#"{"scripts":{"test":"node --test"}}"#);
        let command = self_referential_node_verifier_command(
            root.path(),
            "tests/index.test.js",
            "execFileSync('node', ['src/index.js'], { input: '{\"b\":2}' });",
        );

        assert!(command.is_none());
    }

    #[test]
    fn ignores_comment_only_mentions() {
        let root = tempfile::tempdir().expect("tempdir");
        write_package(root.path(), r#"{"scripts":{"test":"node --test"}}"#);
        let command = self_referential_node_verifier_command(
            root.path(),
            "tests/index.test.js",
            "// execSync('npm test')\nexecFileSync('node', ['src/index.js']);",
        );

        assert!(command.is_none());
    }
}
