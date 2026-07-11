use std::path::Path;

use serde_json::json;

use crate::eval_events;
use crate::planner::step_plan::PlanStep;
use crate::tools::path_guard::validate_workspace_relative;

const SOURCE_ASSERTION_REASON: &str =
    "source implementation detail grep replaced with semantic equivalent assertion";
const SOURCE_ASSERTION_KIND: &str = "source_impl_detail_assertion";
const SOURCE_ASSERTION_MARKER: &str = "__anvil_source_impl_detail_assertion__";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceAssertionRepair {
    pub normalized: String,
    pub reason: String,
    pub kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShellWord {
    value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GrepAssertion {
    pattern: String,
    source_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceAssertionEquivalent {
    KeydownInput,
    ExactToken(String),
}

pub fn normalize_source_assertion_grep(command: &str) -> Option<SourceAssertionRepair> {
    let assertion = source_implementation_assertion(command)?;
    let equivalent = equivalent_for_source_assertion(&assertion.pattern)?;
    Some(SourceAssertionRepair {
        normalized: source_assertion_check_command(&equivalent, &assertion.source_path),
        reason: SOURCE_ASSERTION_REASON.to_string(),
        kind: SOURCE_ASSERTION_KIND,
    })
}

pub fn normalized_command_is_source_assertion(command: &str) -> bool {
    command.contains(SOURCE_ASSERTION_MARKER)
}

pub fn can_demote_failed_source_assertion(
    command: &str,
    step: &PlanStep,
    report_has_prior_failures: bool,
) -> bool {
    normalized_command_is_source_assertion(command)
        && !report_has_prior_failures
        && step_has_contract_or_build_check(step)
}

pub fn emit_demoted_advisory(
    eval_events_path: Option<&Path>,
    command: &str,
    step_id: &str,
    reason: &str,
) {
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "verify_demoted_advisory",
            "command": eval_events::body_snippet(command),
            "step_id": step_id,
            "reason": reason,
        }),
    );
}

pub fn demotion_reason() -> &'static str {
    "source_impl_detail_assertion: equivalent contract/build checks passed; implementation API spelling is advisory"
}

fn source_implementation_assertion(command: &str) -> Option<GrepAssertion> {
    let tokens = shell_words(command)?;
    let mut assertion = grep_assertion(&tokens)?;
    assertion.source_path = source_path(&assertion.source_path)?;
    if is_contract_vocab_pattern(&assertion.pattern)
        || !looks_like_code_api_pattern(&assertion.pattern)
    {
        return None;
    }
    Some(assertion)
}

fn equivalent_for_source_assertion(pattern: &str) -> Option<SourceAssertionEquivalent> {
    let compact = pattern
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    let lower = compact.to_ascii_lowercase();
    if lower.contains("addeventlistener") && lower.contains("keydown")
        || lower.contains("onkeydown")
        || lower.contains("keys[")
    {
        return Some(SourceAssertionEquivalent::KeydownInput);
    }
    source_api_token(pattern).map(SourceAssertionEquivalent::ExactToken)
}

fn source_assertion_check_command(
    equivalent: &SourceAssertionEquivalent,
    source_path: &str,
) -> String {
    match equivalent {
        SourceAssertionEquivalent::KeydownInput => format!(
            concat!(
                "node -p '",
                "(function(s,m){{return [",
                "s.includes(\"onKeyDown\"),",
                "s.includes(\"keys[\"),",
                "s.includes(\"addEventListener\")?s.includes(\"keydown\"):false",
                "].some(function(x){{return x}})?true:process.exit(1)}})",
                "(String(require(\"fs\").readFileSync(\"{source_path}\")),",
                "\"{marker}\")",
                "'"
            ),
            source_path = source_path,
            marker = SOURCE_ASSERTION_MARKER,
        ),
        SourceAssertionEquivalent::ExactToken(token) => format!(
            concat!(
                "node -p '",
                "(function(s,m){{return s.includes(\"{token}\")?true:process.exit(1)}})",
                "(String(require(\"fs\").readFileSync(\"{source_path}\")),",
                "\"{marker}\")",
                "'"
            ),
            token = token,
            source_path = source_path,
            marker = SOURCE_ASSERTION_MARKER,
        ),
    }
}

fn step_has_contract_or_build_check(step: &PlanStep) -> bool {
    step.verify.iter().any(|command| {
        let lower = command.to_ascii_lowercase();
        lower.contains("npm run build")
            || lower.contains("next build")
            || lower.contains("data-anvil-")
            || lower.contains("package.json")
    })
}

fn grep_assertion(tokens: &[ShellWord]) -> Option<GrepAssertion> {
    if tokens.first()?.value != "grep" {
        return None;
    }
    let mut index = 1usize;
    while index < tokens.len() {
        let value = tokens[index].value.as_str();
        if value == "--" {
            let pattern = tokens.get(index + 1)?.value.clone();
            let source_path = tokens.get(index + 2)?.value.clone();
            return (tokens.len() == index + 3).then_some(GrepAssertion {
                pattern,
                source_path,
            });
        }
        if matches!(value, "-e" | "--regexp") {
            let pattern = tokens.get(index + 1)?.value.clone();
            let source_path = tokens.get(index + 2)?.value.clone();
            return (tokens.len() == index + 3).then_some(GrepAssertion {
                pattern,
                source_path,
            });
        }
        if let Some(pattern) = value.strip_prefix("-e").filter(|value| !value.is_empty()) {
            let source_path = tokens.get(index + 1)?.value.clone();
            return (tokens.len() == index + 2).then_some(GrepAssertion {
                pattern: pattern.to_string(),
                source_path,
            });
        }
        if grep_option(value) {
            index += 1;
            continue;
        }
        if value.starts_with('-') {
            return None;
        }
        let pattern = value.to_string();
        let source_path = tokens.get(index + 1)?.value.clone();
        return (tokens.len() == index + 2).then_some(GrepAssertion {
            pattern,
            source_path,
        });
    }
    None
}

fn grep_option(value: &str) -> bool {
    value.starts_with('-')
        && value != "-"
        && value
            .trim_start_matches('-')
            .chars()
            .all(|ch| matches!(ch, 'q' | 's' | 'i' | 'F' | 'E' | 'G' | 'w' | 'x'))
}

fn source_path(path: &str) -> Option<String> {
    let rel = path.trim().trim_start_matches("./").replace('\\', "/");
    if !rel.starts_with("src/")
        || rel.starts_with('/')
        || rel.contains('\0')
        || rel.chars().any(char::is_whitespace)
        || rel.bytes().any(|byte| {
            matches!(
                byte,
                b'\'' | b'"' | b'\\' | b';' | b'&' | b'|' | b'<' | b'>' | b'`'
            )
        })
    {
        return None;
    }
    validate_workspace_relative(&rel).ok()?;
    let ext = Path::new(&rel).extension().and_then(|ext| ext.to_str())?;
    matches!(ext, "tsx" | "ts" | "jsx" | "js" | "mjs" | "cjs").then_some(rel)
}

fn is_contract_vocab_pattern(pattern: &str) -> bool {
    let lower = pattern.to_ascii_lowercase();
    lower.contains("data-anvil-")
        || lower.contains("scripts.")
        || lower.contains("next dev")
        || lower.contains("next start")
        || lower.contains("--port")
        || lower.contains("-p ")
}

fn looks_like_code_api_pattern(pattern: &str) -> bool {
    source_api_token(pattern).is_some()
}

fn source_api_token(pattern: &str) -> Option<String> {
    for token in [
        "addEventListener",
        "removeEventListener",
        "useState",
        "useEffect",
        "onKeyDown",
        "onClick",
        "onChange",
        "keys[",
        "keydown",
    ] {
        if pattern.contains(token) {
            return Some(token.to_string());
        }
    }
    identifier_before_call(pattern)
}

fn identifier_before_call(pattern: &str) -> Option<String> {
    let call_index = pattern.find('(')?;
    let before = pattern[..call_index].trim_end();
    let start = before
        .rfind(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '$'))
        .map(|index| index + 1)
        .unwrap_or(0);
    let token = &before[start..];
    safe_identifier(token).then(|| token.to_string())
}

fn safe_identifier(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_' || first == '$')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
        && !matches!(token, "if" | "for" | "while" | "switch" | "return")
}

fn shell_words(command: &str) -> Option<Vec<ShellWord>> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut single = false;
    let mut double = false;
    let mut started = false;
    for ch in command.chars() {
        if single {
            if ch == '\'' {
                single = false;
            } else {
                current.push(ch);
            }
            continue;
        }
        if double {
            if ch == '"' {
                double = false;
            } else {
                current.push(ch);
            }
            continue;
        }
        if ch.is_whitespace() {
            if started {
                out.push(ShellWord {
                    value: std::mem::take(&mut current),
                });
                started = false;
            }
            continue;
        }
        started = true;
        match ch {
            '\'' => single = true,
            '"' => double = true,
            _ => current.push(ch),
        }
    }
    if single || double {
        return None;
    }
    if started {
        out.push(ShellWord { value: current });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keydown_source_detail_grep_rewrites_to_equivalent_set() {
        let repair = normalize_source_assertion_grep(
            r#"grep -q "addEventListener('keydown'" src/app/page.tsx"#,
        )
        .unwrap();

        assert_eq!(repair.kind, SOURCE_ASSERTION_KIND);
        assert!(repair.normalized.contains(SOURCE_ASSERTION_MARKER));
        assert!(repair.normalized.contains("onKeyDown"));
        assert!(repair.normalized.contains("keys["));
        assert!(repair.normalized.contains("addEventListener"));
        assert!(repair.normalized.contains("keydown"));
    }

    #[test]
    fn contract_hook_grep_is_not_source_detail_assertion() {
        assert!(
            normalize_source_assertion_grep(
                r#"grep -q "data-anvil-action='primary'" src/app/page.tsx"#
            )
            .is_none()
        );
    }

    #[test]
    fn package_and_docs_greps_are_not_source_detail_assertions() {
        assert!(
            normalize_source_assertion_grep(r#"grep -q "next dev -p 3011" package.json"#).is_none()
        );
        assert!(normalize_source_assertion_grep(r#"grep -q "Usage" README.md"#).is_none());
    }
}
