use std::collections::BTreeSet;
use std::path::Path;

use regex::Regex;
use serde_json::json;

use crate::eval_events;
use crate::minimal_loop::import_scan::route_bound_closure;
use crate::planner::verify::VerificationReport;

const STATE_BINDING_CONTRACT: &str = "Minimum contract: after start and after input, the `data-anvil-state` JSON value must actually change. If refs are used, mirror meaningful state changes into React state so a render occurs.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StateBindingDiagnosisKind {
    StateBoundToRef,
    SetterNeverCalled,
    StateReactiveOk,
    Undeterminable,
}

impl StateBindingDiagnosisKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::StateBoundToRef => "state_bound_to_ref",
            Self::SetterNeverCalled => "setter_never_called",
            Self::StateReactiveOk => "state_reactive_ok",
            Self::Undeterminable => "undeterminable",
        }
    }

    fn promptable(self) -> bool {
        matches!(self, Self::StateBoundToRef | Self::SetterNeverCalled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StateBindingDiagnosis {
    pub(crate) diagnosis: StateBindingDiagnosisKind,
    pub(crate) path: String,
    pub(crate) referenced_identifiers: Vec<String>,
    pub(crate) evidence: Vec<String>,
}

impl StateBindingDiagnosis {
    fn undeterminable(path: impl Into<String>, evidence: impl Into<String>) -> Self {
        Self {
            diagnosis: StateBindingDiagnosisKind::Undeterminable,
            path: path.into(),
            referenced_identifiers: Vec::new(),
            evidence: vec![evidence.into()],
        }
    }
}

pub(crate) fn final_acceptance_feedback(
    root: &Path,
    profile: &str,
    report: &VerificationReport,
    eval_events_path: Option<&Path>,
) -> String {
    let mut triggers = Vec::new();
    collect_interaction_state_triggers(&report.primary_reason(), &mut triggers);
    for failure in &report.profile_failures {
        collect_interaction_state_triggers(failure, &mut triggers);
    }
    if triggers.is_empty() {
        return String::new();
    }
    feedback_for_triggered_scan(root, profile, eval_events_path)
}

pub(crate) fn write_required_feedback(
    root: &Path,
    profile: &str,
    missing_evidence: &[String],
    missing_capabilities: &[String],
    eval_events_path: Option<&Path>,
) -> String {
    let triggered = missing_evidence
        .iter()
        .chain(missing_capabilities.iter())
        .any(|value| interaction_state_related(value));
    if !triggered {
        return String::new();
    }
    feedback_for_triggered_scan(root, profile, eval_events_path)
}

fn feedback_for_triggered_scan(
    root: &Path,
    profile: &str,
    eval_events_path: Option<&Path>,
) -> String {
    let diagnosis = diagnose_route_bound_state_binding(root, profile);
    emit_state_binding_diagnosis(eval_events_path, &diagnosis);
    if diagnosis.diagnosis.promptable() {
        state_binding_feedback_for_diagnosis(&diagnosis)
    } else {
        String::new()
    }
}

pub(crate) fn diagnose_route_bound_state_binding(
    root: &Path,
    profile: &str,
) -> StateBindingDiagnosis {
    let mut fallback: Option<StateBindingDiagnosis> = None;
    for rel in route_bound_closure(root, profile) {
        let rel_text = rel.to_string_lossy().replace('\\', "/");
        if !is_source_path(&rel_text) {
            continue;
        }
        let path = root.join(&rel);
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(diagnosis) = diagnose_source_state_binding(&rel_text, &source) else {
            continue;
        };
        if diagnosis.diagnosis.promptable() {
            return diagnosis;
        }
        if fallback.is_none()
            || fallback.as_ref().is_some_and(|existing| {
                existing.diagnosis == StateBindingDiagnosisKind::Undeterminable
                    && diagnosis.diagnosis == StateBindingDiagnosisKind::StateReactiveOk
            })
        {
            fallback = Some(diagnosis);
        }
    }
    fallback.unwrap_or_else(|| {
        StateBindingDiagnosis::undeterminable(
            "",
            "No route-bound data-anvil-state source binding was found.",
        )
    })
}

fn emit_state_binding_diagnosis(
    eval_events_path: Option<&Path>,
    diagnosis: &StateBindingDiagnosis,
) {
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "state_binding_diagnosis",
            "diagnosis": diagnosis.diagnosis.as_str(),
            "path": diagnosis.path.clone(),
            "referenced_identifiers": diagnosis.referenced_identifiers.clone(),
            "evidence": diagnosis
                .evidence
                .iter()
                .map(|line| eval_events::body_snippet(line))
                .collect::<Vec<_>>(),
        }),
    );
}

pub(crate) fn state_binding_feedback_for_diagnosis(diagnosis: &StateBindingDiagnosis) -> String {
    if !diagnosis.diagnosis.promptable() {
        return String::new();
    }
    let mut lines = vec![
        format!("State binding diagnosis: {}", diagnosis.diagnosis.as_str()),
        format!(
            "- route-bound source: {}",
            missing_if_empty(&diagnosis.path)
        ),
    ];
    if diagnosis.referenced_identifiers.is_empty() {
        lines.push("- referenced identifiers: (none)".to_string());
    } else {
        lines.push(format!(
            "- referenced identifiers: {}",
            diagnosis.referenced_identifiers.join(", ")
        ));
    }
    for item in &diagnosis.evidence {
        lines.push(format!("- {item}"));
    }
    lines.push(STATE_BINDING_CONTRACT.to_string());
    lines.join("\n")
}

fn diagnose_source_state_binding(path: &str, source: &str) -> Option<StateBindingDiagnosis> {
    if class_component_like(source) {
        return Some(StateBindingDiagnosis::undeterminable(
            path,
            "Class component or this.setState syntax is outside the conservative scanner.",
        ));
    }
    let expression = extract_state_attribute_expression(source)?;
    if complex_expression(&expression) {
        return Some(StateBindingDiagnosis::undeterminable(
            path,
            "data-anvil-state expression is too complex for conservative diagnosis.",
        ));
    }
    let referenced_identifiers = extract_referenced_identifiers(&expression);
    if referenced_identifiers.is_empty() {
        return Some(StateBindingDiagnosis::undeterminable(
            path,
            "data-anvil-state expression did not expose a referenced state identifier.",
        ));
    }

    let mut reactive = Vec::new();
    let mut non_reactive = Vec::new();
    let mut unknown = Vec::new();
    let mut evidence = Vec::new();
    for identifier in &referenced_identifiers {
        match classify_identifier(source, identifier) {
            IdentifierBinding::UseState(binding) | IdentifierBinding::UseReducer(binding) => {
                evidence.push(format!(
                    "{} declaration: line {} `{}`",
                    identifier, binding.declaration_line, binding.declaration_excerpt
                ));
                if binding.update_lines.is_empty() {
                    evidence.push(format!(
                        "{} update: no {}(...) call found outside the declaration",
                        identifier, binding.updater
                    ));
                } else {
                    for update in &binding.update_lines {
                        evidence.push(format!(
                            "{} update: line {} `{}`",
                            identifier, update.line, update.excerpt
                        ));
                    }
                }
                reactive.push(binding);
            }
            IdentifierBinding::UseRef(binding) | IdentifierBinding::Plain(binding) => {
                evidence.push(format!(
                    "{} declaration: line {} `{}`",
                    identifier, binding.declaration_line, binding.declaration_excerpt
                ));
                let mutations = ref_or_plain_update_lines(source, identifier);
                if mutations.is_empty() {
                    evidence.push(format!(
                        "{} update: no React state setter mirrors this value",
                        identifier
                    ));
                } else {
                    for update in mutations {
                        evidence.push(format!(
                            "{} non-reactive update: line {} `{}`",
                            identifier, update.line, update.excerpt
                        ));
                    }
                }
                non_reactive.push(binding);
            }
            IdentifierBinding::Unknown => unknown.push(identifier.clone()),
        }
    }

    if !unknown.is_empty() {
        evidence.push(format!("undetermined declarations: {}", unknown.join(", ")));
        return Some(StateBindingDiagnosis {
            diagnosis: StateBindingDiagnosisKind::Undeterminable,
            path: path.to_string(),
            referenced_identifiers,
            evidence,
        });
    }

    let reactive_updates = reactive
        .iter()
        .flat_map(|binding| binding.update_lines.iter())
        .collect::<Vec<_>>();
    if !reactive.is_empty() && reactive_updates.is_empty() {
        return Some(StateBindingDiagnosis {
            diagnosis: StateBindingDiagnosisKind::SetterNeverCalled,
            path: path.to_string(),
            referenced_identifiers,
            evidence,
        });
    }
    if !reactive_updates.is_empty() {
        if reactive_updates
            .iter()
            .any(|update| update.reactive_context)
        {
            return Some(StateBindingDiagnosis {
                diagnosis: StateBindingDiagnosisKind::StateReactiveOk,
                path: path.to_string(),
                referenced_identifiers,
                evidence,
            });
        }
        evidence.push(
            "setter or dispatch call exists, but not in a recognized start/restart/input/loop context"
                .to_string(),
        );
        return Some(StateBindingDiagnosis {
            diagnosis: StateBindingDiagnosisKind::Undeterminable,
            path: path.to_string(),
            referenced_identifiers,
            evidence,
        });
    }
    if !non_reactive.is_empty() {
        return Some(StateBindingDiagnosis {
            diagnosis: StateBindingDiagnosisKind::StateBoundToRef,
            path: path.to_string(),
            referenced_identifiers,
            evidence,
        });
    }
    Some(StateBindingDiagnosis {
        diagnosis: StateBindingDiagnosisKind::Undeterminable,
        path: path.to_string(),
        referenced_identifiers,
        evidence,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReactiveBinding {
    updater: String,
    declaration_line: usize,
    declaration_excerpt: String,
    update_lines: Vec<UpdateLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NonReactiveBinding {
    declaration_line: usize,
    declaration_excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpdateLine {
    line: usize,
    excerpt: String,
    reactive_context: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IdentifierBinding {
    UseState(ReactiveBinding),
    UseReducer(ReactiveBinding),
    UseRef(NonReactiveBinding),
    Plain(NonReactiveBinding),
    Unknown,
}

fn classify_identifier(source: &str, identifier: &str) -> IdentifierBinding {
    if let Some((updater, byte)) = capture_reactive_declaration(source, identifier, "useState") {
        return IdentifierBinding::UseState(ReactiveBinding {
            update_lines: updater_update_lines(source, &updater, byte),
            updater,
            declaration_line: line_number_at(source, byte),
            declaration_excerpt: line_excerpt_at(source, byte),
        });
    }
    if let Some((updater, byte)) = capture_reactive_declaration(source, identifier, "useReducer") {
        return IdentifierBinding::UseReducer(ReactiveBinding {
            update_lines: updater_update_lines(source, &updater, byte),
            updater,
            declaration_line: line_number_at(source, byte),
            declaration_excerpt: line_excerpt_at(source, byte),
        });
    }
    if let Some(byte) = capture_simple_declaration(source, identifier, "useRef") {
        return IdentifierBinding::UseRef(NonReactiveBinding {
            declaration_line: line_number_at(source, byte),
            declaration_excerpt: line_excerpt_at(source, byte),
        });
    }
    if let Some(byte) = capture_plain_declaration(source, identifier) {
        return IdentifierBinding::Plain(NonReactiveBinding {
            declaration_line: line_number_at(source, byte),
            declaration_excerpt: line_excerpt_at(source, byte),
        });
    }
    IdentifierBinding::Unknown
}

fn capture_reactive_declaration(
    source: &str,
    identifier: &str,
    hook_name: &str,
) -> Option<(String, usize)> {
    let pattern = format!(
        r#"(?m)\b(?:const|let|var)\s*\[\s*{}\s*,\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*\]\s*=\s*(?:React\.)?{}\s*\("#,
        regex::escape(identifier),
        hook_name
    );
    let captures = Regex::new(&pattern).ok()?.captures(source)?;
    let full = captures.get(0)?;
    let updater = captures.get(1)?.as_str().to_string();
    Some((updater, full.start()))
}

fn capture_simple_declaration(source: &str, identifier: &str, hook_name: &str) -> Option<usize> {
    let pattern = format!(
        r#"(?m)\b(?:const|let|var)\s+{}\s*=\s*(?:React\.)?{}\s*\("#,
        regex::escape(identifier),
        hook_name
    );
    Regex::new(&pattern)
        .ok()?
        .find(source)
        .map(|matched| matched.start())
}

fn capture_plain_declaration(source: &str, identifier: &str) -> Option<usize> {
    let pattern = format!(
        r#"(?m)\b(?:const|let|var)\s+{}\b"#,
        regex::escape(identifier)
    );
    Regex::new(&pattern)
        .ok()?
        .find(source)
        .map(|matched| matched.start())
}

fn updater_update_lines(source: &str, updater: &str, declaration_byte: usize) -> Vec<UpdateLine> {
    let pattern = format!(r#"\b{}\s*\("#, regex::escape(updater));
    let Ok(regex) = Regex::new(&pattern) else {
        return Vec::new();
    };
    regex
        .find_iter(source)
        .filter(|matched| matched.start() != declaration_byte)
        .filter(|matched| !line_excerpt_at(source, matched.start()).contains("useState"))
        .filter(|matched| !line_excerpt_at(source, matched.start()).contains("useReducer"))
        .map(|matched| UpdateLine {
            line: line_number_at(source, matched.start()),
            excerpt: line_excerpt_at(source, matched.start()),
            reactive_context: setter_call_has_reactive_context(source, matched.start()),
        })
        .collect()
}

fn setter_call_has_reactive_context(source: &str, byte: usize) -> bool {
    let line = line_number_at(source, byte);
    let context = surrounding_lines(source, line, 4).to_ascii_lowercase();
    [
        "onclick",
        "onchange",
        "oninput",
        "onkeydown",
        "onkeyup",
        "onpointer",
        "onmouse",
        "addeventlistener",
        "requestanimationframe",
        "setinterval",
        "settimeout",
        "function start",
        "function restart",
        "const start",
        "const restart",
        "let start",
        "let restart",
        "handleinput",
        "handlekey",
        "handlestart",
        "handlerestart",
        "gameloop",
        "tick",
        "loop",
    ]
    .iter()
    .any(|needle| context.contains(needle))
}

fn ref_or_plain_update_lines(source: &str, identifier: &str) -> Vec<UpdateLine> {
    source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim();
            let mut mentions_update = trimmed.contains(&format!("{identifier}.current"))
                || trimmed.contains(&format!("{identifier} ="))
                || trimmed.contains(&format!("{identifier} +="))
                || trimmed.contains(&format!("{identifier}++"));
            mentions_update &= trimmed.contains('=')
                || trimmed.contains("+=")
                || trimmed.contains("++")
                || trimmed.contains(".push(")
                || trimmed.contains(".splice(");
            mentions_update.then(|| UpdateLine {
                line: index + 1,
                excerpt: trimmed.to_string(),
                reactive_context: false,
            })
        })
        .collect()
}

fn extract_state_attribute_expression(source: &str) -> Option<String> {
    let attr = "data-anvil-state";
    let start = source.find(attr)?;
    let after_attr = start + attr.len();
    let after_equals = source[after_attr..].find('=')? + after_attr + 1;
    let bytes = source.as_bytes();
    let mut index = after_equals;
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    if bytes.get(index).copied() != Some(b'{') {
        return None;
    }
    balanced_jsx_brace_expression(source, index)
}

fn balanced_jsx_brace_expression(source: &str, open_byte: usize) -> Option<String> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut in_string: Option<u8> = None;
    let mut escaped = false;
    for index in open_byte..bytes.len() {
        let byte = bytes[index];
        if let Some(quote) = in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == quote {
                in_string = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' | b'`' => in_string = Some(byte),
            b'{' => depth = depth.saturating_add(1),
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(source[open_byte + 1..index].trim().to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn extract_referenced_identifiers(expression: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    let bytes = expression.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if !identifier_start(bytes[index]) {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < bytes.len() && identifier_continue(bytes[index]) {
            index += 1;
        }
        let identifier = &expression[start..index];
        if ignored_identifier(identifier)
            || previous_significant_byte(expression, start) == Some(b'.')
            || next_significant_byte(expression, index) == Some(b':')
        {
            continue;
        }
        out.insert(identifier.to_string());
    }
    out.into_iter().collect()
}

fn complex_expression(expression: &str) -> bool {
    let lower = expression.to_ascii_lowercase();
    expression.contains("=>")
        || lower.contains("function")
        || lower.contains("this.")
        || expression.contains('?')
        || expression.contains('[')
}

fn class_component_like(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    (lower.contains("class ") && lower.contains(" extends ")) || lower.contains("this.setstate")
}

fn collect_interaction_state_triggers(text: &str, out: &mut Vec<String>) {
    for token in text
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | ';' | ')' | ']' | '('))
        .map(|token| token.trim_matches(|ch: char| matches!(ch, '.' | ':' | '"' | '\'')))
        .filter(|token| interaction_state_related(token))
    {
        if !out.iter().any(|existing| existing == token) {
            out.push(token.to_string());
        }
    }
    if interaction_state_related(text) && out.is_empty() {
        out.push(text.to_string());
    }
}

fn interaction_state_related(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "input_state_change_missing_after_start",
        "input_state_change_not_evaluated_after_start",
        "interaction_state_change_missing",
        "text_input_state_change_missing",
        "stateful_update_evidence",
        "restart_or_recoverable_state_evidence",
        "user_input_handler_evidence",
        "stateful_interaction",
        "visible_state_change",
        "playable_ui",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn ignored_identifier(identifier: &str) -> bool {
    matches!(
        identifier,
        "JSON"
            | "stringify"
            | "parse"
            | "Math"
            | "Number"
            | "String"
            | "Boolean"
            | "Array"
            | "Object"
            | "true"
            | "false"
            | "null"
            | "undefined"
            | "NaN"
            | "Infinity"
    )
}

fn previous_significant_byte(value: &str, byte: usize) -> Option<u8> {
    value.as_bytes()[..byte]
        .iter()
        .rev()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
}

fn next_significant_byte(value: &str, byte: usize) -> Option<u8> {
    value.as_bytes()[byte..]
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
}

fn identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$')
}

fn identifier_continue(byte: u8) -> bool {
    identifier_start(byte) || byte.is_ascii_digit()
}

fn line_number_at(source: &str, byte: usize) -> usize {
    source.as_bytes()[..byte.min(source.len())]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}

fn line_excerpt_at(source: &str, byte: usize) -> String {
    let line = line_number_at(source, byte);
    source
        .lines()
        .nth(line.saturating_sub(1))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn surrounding_lines(source: &str, line: usize, radius: usize) -> String {
    let start = line.saturating_sub(radius + 1);
    let end = line.saturating_add(radius);
    source
        .lines()
        .enumerate()
        .filter(|(index, _)| *index >= start && *index < end)
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_source_path(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|ext| {
        matches!(
            ext.to_str().unwrap_or_default(),
            "js" | "jsx" | "ts" | "tsx"
        )
    })
}

fn missing_if_empty(value: &str) -> &str {
    if value.trim().is_empty() {
        "(unknown)"
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_ref_raf_loop_without_set_state_is_state_bound_to_ref() {
        let source = r#"
import { useEffect, useRef } from "react";
export default function Game() {
  const gameRef = useRef({ score: 0 });
  useEffect(() => {
    const tick = () => {
      gameRef.current.score += 1;
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }, []);
  return <main data-anvil-state={JSON.stringify({ score: gameRef.current.score })} />;
}
"#;
        let diagnosis = diagnose_source_state_binding("src/app/page.tsx", source).unwrap();
        assert_eq!(
            diagnosis.diagnosis,
            StateBindingDiagnosisKind::StateBoundToRef
        );
        assert_eq!(diagnosis.referenced_identifiers, vec!["gameRef"]);
    }

    #[test]
    fn use_state_binding_without_setter_call_is_setter_never_called() {
        let source = r#"
import { useState } from "react";
export default function Game() {
  const [score, setScore] = useState(0);
  return <main data-anvil-state={JSON.stringify({ score })} />;
}
"#;
        let diagnosis = diagnose_source_state_binding("src/app/page.tsx", source).unwrap();
        assert_eq!(
            diagnosis.diagnosis,
            StateBindingDiagnosisKind::SetterNeverCalled
        );
        assert!(
            diagnosis
                .evidence
                .iter()
                .any(|line| line.contains("setScore"))
        );
    }

    #[test]
    fn use_state_handler_setter_call_is_state_reactive_ok() {
        let source = r#"
import { useState } from "react";
export default function Game() {
  const [score, setScore] = useState(0);
  const start = () => {
    setScore((value) => value + 1);
  };
  return <main data-anvil-state={JSON.stringify({ score })}>
    <button data-anvil-action="primary" onClick={start}>Start</button>
  </main>;
}
"#;
        let diagnosis = diagnose_source_state_binding("src/app/page.tsx", source).unwrap();
        assert_eq!(
            diagnosis.diagnosis,
            StateBindingDiagnosisKind::StateReactiveOk
        );
    }

    #[test]
    fn class_component_is_undeterminable() {
        let source = r#"
class Game extends React.Component {
  render() {
    return <main data-anvil-state={JSON.stringify(this.state)} />;
  }
}
"#;
        let diagnosis = diagnose_source_state_binding("src/app/page.tsx", source).unwrap();
        assert_eq!(
            diagnosis.diagnosis,
            StateBindingDiagnosisKind::Undeterminable
        );
    }

    #[test]
    fn final_acceptance_feedback_requires_interaction_state_trigger() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src/app");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("page.tsx"),
            r#"
import { useState } from "react";
export default function Game() {
  const [score, setScore] = useState(0);
  return <main data-anvil-state={JSON.stringify({ score })} />;
}
"#,
        )
        .unwrap();
        let report = VerificationReport::profile_failed(
            "browser_interaction_failed:input_state_change_missing_after_start",
        );
        let feedback = final_acceptance_feedback(dir.path(), "nextjs", &report, None);
        assert!(feedback.contains("State binding diagnosis: setter_never_called"));
        assert!(feedback.contains("Minimum contract: after start and after input"));

        let build_report = VerificationReport::command_failed(
            "npm run build",
            "implementation_compile_error: TS2304",
        );
        let feedback = final_acceptance_feedback(dir.path(), "nextjs", &build_report, None);
        assert!(feedback.is_empty(), "{feedback}");
    }

    #[test]
    fn write_required_feedback_uses_same_interaction_trigger() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src/app");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("page.tsx"),
            r#"
import { useRef } from "react";
export default function Game() {
  const gameRef = useRef({ score: 0 });
  return <main data-anvil-state={JSON.stringify({ score: gameRef.current.score })} />;
}
"#,
        )
        .unwrap();
        let feedback = write_required_feedback(
            dir.path(),
            "nextjs",
            &["stateful_update_evidence".to_string()],
            &[],
            None,
        );
        assert!(feedback.contains("State binding diagnosis: state_bound_to_ref"));
    }
}
