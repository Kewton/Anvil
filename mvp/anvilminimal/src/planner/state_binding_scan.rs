use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::json;

use crate::eval_events;
use crate::minimal_loop::import_scan::route_bound_closure;
use crate::planner::verify::VerificationReport;

const STATE_BINDING_CONTRACT: &str = "Minimum contract: after start and after input, the `data-anvil-state` JSON value must actually change. The snapshot must include at least one dimension that immediately responds to input, such as player/paddle x position. If refs are used, mirror meaningful state changes into React state so a render occurs.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StateBindingDiagnosisKind {
    StateBoundToRef,
    SetterNeverCalled,
    InputCoupledStateNotSnapshotted,
    StateReactiveOk,
    Undeterminable,
}

impl StateBindingDiagnosisKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::StateBoundToRef => "state_bound_to_ref",
            Self::SetterNeverCalled => "setter_never_called",
            Self::InputCoupledStateNotSnapshotted => "input_coupled_state_not_snapshotted",
            Self::StateReactiveOk => "state_reactive_ok",
            Self::Undeterminable => "undeterminable",
        }
    }

    fn actionable(self) -> bool {
        matches!(
            self,
            Self::StateBoundToRef | Self::SetterNeverCalled | Self::InputCoupledStateNotSnapshotted
        )
    }

    fn feedback_worthy(self) -> bool {
        self.actionable() || self == Self::Undeterminable
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UndeterminableReason {
    GenericDeclarationUnmatched,
    UnknownIdentifiers,
    ComplexExpression,
    NoExpression,
}

impl UndeterminableReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::GenericDeclarationUnmatched => "generic_declaration_unmatched",
            Self::UnknownIdentifiers => "unknown_identifiers",
            Self::ComplexExpression => "complex_expression",
            Self::NoExpression => "no_expression",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StateBindingDiagnosis {
    pub(crate) diagnosis: StateBindingDiagnosisKind,
    pub(crate) path: String,
    pub(crate) referenced_identifiers: Vec<String>,
    pub(crate) evidence: Vec<String>,
    pub(crate) input_mutation_refs: Vec<String>,
    pub(crate) undeterminable_reason: Option<UndeterminableReason>,
}

impl StateBindingDiagnosis {
    fn undeterminable(
        path: impl Into<String>,
        evidence: impl Into<String>,
        reason: UndeterminableReason,
    ) -> Self {
        Self {
            diagnosis: StateBindingDiagnosisKind::Undeterminable,
            path: path.into(),
            referenced_identifiers: Vec::new(),
            evidence: vec![evidence.into()],
            input_mutation_refs: Vec::new(),
            undeterminable_reason: Some(reason),
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
    if !diagnosis.diagnosis.feedback_worthy() {
        return String::new();
    }
    let mut feedback = state_binding_feedback_for_diagnosis(&diagnosis);
    if let Some(issue) = contract_attribute_issue_for_no_expression(&diagnosis) {
        let guidance = crate::planner::contract_attribute_repair::guidance_for_issue(
            Some(root),
            &issue,
            eval_events_path,
        );
        if !guidance.is_empty() {
            feedback.push_str("\n\n");
            feedback.push_str(&guidance);
        }
    }
    feedback
}

pub(crate) fn diagnose_route_bound_state_binding(
    root: &Path,
    profile: &str,
) -> StateBindingDiagnosis {
    let mut fallback: Option<StateBindingDiagnosis> = None;
    for rel in state_binding_scan_paths(root, profile) {
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
        if diagnosis.diagnosis.actionable() {
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
            UndeterminableReason::NoExpression,
        )
    })
}

fn contract_attribute_issue_for_no_expression(
    diagnosis: &StateBindingDiagnosis,
) -> Option<crate::planner::contract_attribute_repair::ContractAttributeIssue> {
    if diagnosis.diagnosis != StateBindingDiagnosisKind::Undeterminable
        || diagnosis.undeterminable_reason != Some(UndeterminableReason::NoExpression)
        || diagnosis.path.trim().is_empty()
    {
        return None;
    }
    Some(
        crate::planner::contract_attribute_repair::ContractAttributeIssue {
            attribute: "data-anvil-state".to_string(),
            path: diagnosis.path.clone(),
        },
    )
}

fn state_binding_scan_paths(root: &Path, profile: &str) -> Vec<PathBuf> {
    let mut route_entries = Vec::new();
    let mut non_layout = Vec::new();
    for rel in route_bound_closure(root, profile) {
        let rel_text = rel.to_string_lossy().replace('\\', "/");
        if layout_source_path(&rel_text) {
            continue;
        }
        if route_entry_source_path(&rel_text) {
            route_entries.push(rel);
        } else {
            non_layout.push(rel);
        }
    }
    route_entries.sort();
    non_layout.sort();
    route_entries.extend(non_layout);
    route_entries
}

fn route_entry_source_path(path: &str) -> bool {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "page.tsx"
            | "page.ts"
            | "page.jsx"
            | "page.js"
            | "index.tsx"
            | "index.ts"
            | "index.jsx"
            | "index.js"
    )
}

fn layout_source_path(path: &str) -> bool {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "layout.tsx" | "layout.ts" | "layout.jsx" | "layout.js"
    )
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
            "input_mutation_refs": diagnosis.input_mutation_refs.clone(),
            "undeterminable_reason": diagnosis
                .undeterminable_reason
                .map(UndeterminableReason::as_str)
                .unwrap_or(""),
            "evidence": diagnosis
                .evidence
                .iter()
                .map(|line| eval_events::body_snippet(line))
                .collect::<Vec<_>>(),
        }),
    );
}

pub(crate) fn state_binding_feedback_for_diagnosis(diagnosis: &StateBindingDiagnosis) -> String {
    if !diagnosis.diagnosis.feedback_worthy() {
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
    if diagnosis.diagnosis == StateBindingDiagnosisKind::InputCoupledStateNotSnapshotted {
        lines.push(format!(
            "入力は {} を変異させるが data-anvil-state に映らない。入力連動次元（例: プレイヤー/パドルのx座標）をReact stateにミラーしてスナップショットへ追加せよ。",
            if diagnosis.input_mutation_refs.is_empty() {
                "(unknown ref)".to_string()
            } else {
                diagnosis.input_mutation_refs.join(", ")
            }
        ));
    }
    lines.push(STATE_BINDING_CONTRACT.to_string());
    lines.join("\n")
}

fn diagnose_source_state_binding(path: &str, source: &str) -> Option<StateBindingDiagnosis> {
    if class_component_like(source) {
        return Some(StateBindingDiagnosis::undeterminable(
            path,
            "Class component or this.setState syntax is outside the conservative scanner.",
            UndeterminableReason::ComplexExpression,
        ));
    }
    let expression = match extract_state_attribute_expression(source) {
        Some(expression) => expression,
        None => {
            return Some(StateBindingDiagnosis::undeterminable(
                path,
                "No data-anvil-state expression was found.",
                UndeterminableReason::NoExpression,
            ));
        }
    };
    if complex_expression(&expression) {
        let referenced_identifiers = extract_referenced_identifiers(&expression);
        let mut evidence = vec![
            "data-anvil-state expression is too complex for conservative diagnosis.".to_string(),
        ];
        append_identifier_declaration_facts(source, &referenced_identifiers, &mut evidence);
        return Some(StateBindingDiagnosis {
            diagnosis: StateBindingDiagnosisKind::Undeterminable,
            path: path.to_string(),
            referenced_identifiers,
            evidence,
            input_mutation_refs: Vec::new(),
            undeterminable_reason: Some(UndeterminableReason::ComplexExpression),
        });
    }
    let referenced_identifiers = extract_referenced_identifiers(&expression);
    if referenced_identifiers.is_empty() {
        return Some(StateBindingDiagnosis::undeterminable(
            path,
            "data-anvil-state expression did not expose a referenced state identifier.",
            UndeterminableReason::NoExpression,
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
            IdentifierBinding::Unknown => {
                append_identifier_declaration_fact(source, identifier, &mut evidence);
                unknown.push(identifier.clone());
            }
        }
    }

    if !unknown.is_empty() {
        evidence.push(format!("undetermined declarations: {}", unknown.join(", ")));
        let undeterminable_reason = if unknown
            .iter()
            .any(|identifier| generic_hook_declaration_unmatched(source, identifier))
        {
            UndeterminableReason::GenericDeclarationUnmatched
        } else {
            UndeterminableReason::UnknownIdentifiers
        };
        return Some(StateBindingDiagnosis {
            diagnosis: StateBindingDiagnosisKind::Undeterminable,
            path: path.to_string(),
            referenced_identifiers,
            evidence,
            input_mutation_refs: Vec::new(),
            undeterminable_reason: Some(undeterminable_reason),
        });
    }

    if !reactive.is_empty()
        && non_reactive.is_empty()
        && let Some(input_coupled) = input_coupled_state_not_snapshotted(source, &reactive)
    {
        evidence.extend(input_coupled.evidence);
        return Some(StateBindingDiagnosis {
            diagnosis: StateBindingDiagnosisKind::InputCoupledStateNotSnapshotted,
            path: path.to_string(),
            referenced_identifiers,
            evidence,
            input_mutation_refs: input_coupled.ref_names,
            undeterminable_reason: None,
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
            input_mutation_refs: Vec::new(),
            undeterminable_reason: None,
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
                input_mutation_refs: Vec::new(),
                undeterminable_reason: None,
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
            input_mutation_refs: Vec::new(),
            undeterminable_reason: Some(UndeterminableReason::ComplexExpression),
        });
    }
    if !non_reactive.is_empty() {
        return Some(StateBindingDiagnosis {
            diagnosis: StateBindingDiagnosisKind::StateBoundToRef,
            path: path.to_string(),
            referenced_identifiers,
            evidence,
            input_mutation_refs: Vec::new(),
            undeterminable_reason: None,
        });
    }
    Some(StateBindingDiagnosis {
        diagnosis: StateBindingDiagnosisKind::Undeterminable,
        path: path.to_string(),
        referenced_identifiers,
        evidence,
        input_mutation_refs: Vec::new(),
        undeterminable_reason: Some(UndeterminableReason::UnknownIdentifiers),
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
struct InputCoupledStateScan {
    ref_names: Vec<String>,
    evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RefPositionMutation {
    ref_name: String,
    line: usize,
    excerpt: String,
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
        r#"(?m)\b(?:const|let|var)\s*\[\s*{}\s*,\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*\]\s*=\s*(?:React\.)?{}\s*(?:<[^>]*>)?\s*\("#,
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
        r#"(?m)\b(?:const|let|var)\s+{}\s*=\s*(?:React\.)?{}\s*(?:<[^>]*>)?\s*\("#,
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

fn append_identifier_declaration_facts(
    source: &str,
    identifiers: &[String],
    evidence: &mut Vec<String>,
) {
    for identifier in identifiers {
        append_identifier_declaration_fact(source, identifier, evidence);
    }
}

fn append_identifier_declaration_fact(source: &str, identifier: &str, evidence: &mut Vec<String>) {
    if evidence
        .iter()
        .any(|line| line.starts_with(&format!("{identifier} declaration:")))
    {
        return;
    }
    if let Some(byte) = declaration_candidate(source, identifier) {
        evidence.push(format!(
            "{} declaration: line {} `{}`",
            identifier,
            line_number_at(source, byte),
            line_excerpt_at(source, byte)
        ));
    } else {
        evidence.push(format!("{identifier} declaration: not found"));
    }
}

fn declaration_candidate(source: &str, identifier: &str) -> Option<usize> {
    let pattern = format!(
        r#"(?m)\b(?:const|let|var)\s+(?:\[\s*{}\b|{}\b)"#,
        regex::escape(identifier),
        regex::escape(identifier)
    );
    Regex::new(&pattern)
        .ok()?
        .find(source)
        .map(|matched| matched.start())
}

fn generic_hook_declaration_unmatched(source: &str, identifier: &str) -> bool {
    let pattern = format!(
        r#"(?m)\b(?:const|let|var)\s*\[\s*{}\b[^\]]*\]\s*=\s*(?:React\.)?(?:useState|useReducer)\s*<"#,
        regex::escape(identifier)
    );
    Regex::new(&pattern).is_ok_and(|regex| regex.is_match(source))
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

fn input_coupled_state_not_snapshotted(
    source: &str,
    snapshot_reactive: &[ReactiveBinding],
) -> Option<InputCoupledStateScan> {
    if !source_has_input_key_path(source) {
        return None;
    }
    let mutations = input_position_ref_mutations(source);
    if mutations.is_empty()
        || snapshot_setter_called_near_mutation(source, snapshot_reactive, &mutations)
    {
        return None;
    }

    let mut ref_names = mutations
        .iter()
        .map(|mutation| mutation.ref_name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    ref_names.sort();

    let mut evidence = Vec::new();
    for ref_name in &ref_names {
        append_identifier_declaration_fact(source, ref_name, &mut evidence);
    }
    for mutation in &mutations {
        evidence.push(format!(
            "input-coupled ref mutation: line {} `{}`",
            mutation.line, mutation.excerpt
        ));
    }
    Some(InputCoupledStateScan {
        ref_names,
        evidence,
    })
}

fn source_has_input_key_path(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    [
        "keydown",
        "keyup",
        "keypressed",
        "keyspressed",
        "keys[",
        ".keys",
        "arrowleft",
        "arrowright",
        "arrowup",
        "arrowdown",
        "keya",
        "keyd",
        "space",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn snapshot_setter_called_near_mutation(
    source: &str,
    snapshot_reactive: &[ReactiveBinding],
    mutations: &[RefPositionMutation],
) -> bool {
    mutations.iter().any(|mutation| {
        let context = surrounding_lines(source, mutation.line, 3);
        snapshot_reactive.iter().any(|binding| {
            let setter_call = format!("{}(", binding.updater);
            context.contains(&setter_call)
        })
    })
}

fn input_position_ref_mutations(source: &str) -> Vec<RefPositionMutation> {
    let aliases = destructured_ref_aliases(source);
    let mut mutations = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let line_number = index + 1;
        let context = surrounding_lines(source, line_number, 3);
        if !line_has_position_assignment(line) || !input_context(&context) {
            continue;
        }
        if let Some(ref_name) = direct_ref_current_mutation_ref(line) {
            mutations.push(RefPositionMutation {
                ref_name,
                line: line_number,
                excerpt: line.trim().to_string(),
            });
            continue;
        }
        if let Some(ref_name) = alias_position_mutation_ref(line, &aliases) {
            mutations.push(RefPositionMutation {
                ref_name,
                line: line_number,
                excerpt: line.trim().to_string(),
            });
        }
    }
    dedup_ref_mutations(mutations)
}

fn destructured_ref_aliases(source: &str) -> Vec<(String, String)> {
    let Ok(destructured) =
        Regex::new(r#"(?m)\bconst\s*\{([^}]+)\}\s*=\s*([A-Za-z_$][A-Za-z0-9_$]*)\.current\b"#)
    else {
        return Vec::new();
    };
    let mut aliases = Vec::new();
    for captures in destructured.captures_iter(source) {
        let Some(fields) = captures.get(1).map(|matched| matched.as_str()) else {
            continue;
        };
        let Some(ref_name) = captures.get(2).map(|matched| matched.as_str().to_string()) else {
            continue;
        };
        for field in fields.split(',') {
            let alias = field
                .split(':')
                .next_back()
                .unwrap_or(field)
                .trim()
                .trim_start_matches("...")
                .trim();
            if identifier_like(alias) {
                aliases.push((alias.to_string(), ref_name.clone()));
            }
        }
    }
    let Ok(simple) = Regex::new(
        r#"(?m)\bconst\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*=\s*([A-Za-z_$][A-Za-z0-9_$]*)\.current\.[A-Za-z_$][A-Za-z0-9_$]*\b"#,
    ) else {
        return aliases;
    };
    for captures in simple.captures_iter(source) {
        if let (Some(alias), Some(ref_name)) = (captures.get(1), captures.get(2)) {
            aliases.push((alias.as_str().to_string(), ref_name.as_str().to_string()));
        }
    }
    aliases
}

fn input_context(context: &str) -> bool {
    let lower = context.to_ascii_lowercase();
    [
        "keydown",
        "keyup",
        "keypressed",
        "keyspressed",
        "keys[",
        ".keys",
        "arrowleft",
        "arrowright",
        "arrowup",
        "arrowdown",
        "keya",
        "keyd",
        "space",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn line_has_position_assignment(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let has_position = [
        ".x",
        ".y",
        ".position",
        ".playerx",
        ".playery",
        ".paddlex",
        ".paddley",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    has_position
        && (line.contains(" =")
            || line.contains("=")
            || line.contains("+=")
            || line.contains("-=")
            || line.contains("++")
            || line.contains("--"))
}

fn direct_ref_current_mutation_ref(line: &str) -> Option<String> {
    let regex = Regex::new(
        r#"\b([A-Za-z_$][A-Za-z0-9_$]*)\.current(?:\.[A-Za-z_$][A-Za-z0-9_$]*)*\.(?:x|y|position|playerX|playerY|paddleX|paddleY)\s*(?:=|\+=|-=|\+\+|--)"#,
    )
    .ok()?;
    regex
        .captures(line)
        .and_then(|captures| captures.get(1).map(|matched| matched.as_str().to_string()))
}

fn alias_position_mutation_ref(line: &str, aliases: &[(String, String)]) -> Option<String> {
    aliases.iter().find_map(|(alias, ref_name)| {
        let pattern = format!(
            r#"\b{}\.(?:x|y|position|playerX|playerY|paddleX|paddleY)\s*(?:=|\+=|-=|\+\+|--)"#,
            regex::escape(alias)
        );
        Regex::new(&pattern)
            .ok()
            .filter(|regex| regex.is_match(line))
            .map(|_| ref_name.clone())
    })
}

fn dedup_ref_mutations(mutations: Vec<RefPositionMutation>) -> Vec<RefPositionMutation> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for mutation in mutations {
        let key = (
            mutation.ref_name.clone(),
            mutation.line,
            mutation.excerpt.clone(),
        );
        if seen.insert(key) {
            out.push(mutation);
        }
    }
    out
}

fn identifier_like(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || matches!(first, '_' | '$'))
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$'))
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
    fn embedded_contract_knowledge_matches_legacy_state_binding_text() {
        let contracts = &crate::planner::profiles::nextjs::knowledge::get().contracts;
        assert_eq!(contracts.state_binding_contract, STATE_BINDING_CONTRACT);
        assert_eq!(
            contracts.input_coupled_dimension_requirement,
            "入力は {mutation_refs} を変異させるが data-anvil-state に映らない。入力連動次元（例: プレイヤー/パドルのx座標）をReact stateにミラーしてスナップショットへ追加せよ。"
        );
        assert!(
            contracts
                .state_binding_contract
                .contains("at least one dimension that immediately responds to input")
        );
    }

    fn fixture_source(name: &str) -> String {
        std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/state_binding")
                .join(name),
        )
        .unwrap()
    }

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
    fn use_state_generic_declarations_are_captured() {
        let source = r#"
type GameState = 'A' | 'B';
export default function Game() {
  const [gameState, setGameState] = useState<GameState>('A');
  const [mode, setMode] = React.useState<'A' | 'B'>('A');
  const flagRef = useRef<HTMLCanvasElement>(null);
  return <main data-anvil-state={JSON.stringify({ gameState, mode })} />;
}
"#;

        assert!(matches!(
            classify_identifier(source, "gameState"),
            IdentifierBinding::UseState(_)
        ));
        assert!(matches!(
            classify_identifier(source, "mode"),
            IdentifierBinding::UseState(_)
        ));
        assert!(matches!(
            classify_identifier(source, "flagRef"),
            IdentifierBinding::UseRef(_)
        ));
    }

    #[test]
    fn real_state_binding_fixtures_are_input_coupled_not_snapshotted() {
        for fixture in ["bs005_combo2_game.tsx", "bs006_space_combo2_page.tsx"] {
            let source = fixture_source(fixture);
            assert!(matches!(
                classify_identifier(&source, "gameState"),
                IdentifierBinding::UseState(_)
            ));

            let diagnosis = diagnose_source_state_binding("src/app/page.tsx", &source).unwrap();

            assert_eq!(
                diagnosis.diagnosis,
                StateBindingDiagnosisKind::InputCoupledStateNotSnapshotted,
                "{fixture}: {diagnosis:?}"
            );
            assert!(
                diagnosis
                    .evidence
                    .iter()
                    .any(|line| line.contains("input-coupled ref mutation")),
                "{diagnosis:?}"
            );
            assert!(
                diagnosis
                    .input_mutation_refs
                    .iter()
                    .any(|name| { name == "playerRef" || name == "gameRef" }),
                "{diagnosis:?}"
            );
            let feedback = state_binding_feedback_for_diagnosis(&diagnosis);
            assert!(feedback.contains("入力は"), "{feedback}");
            assert!(feedback.contains("data-anvil-state"), "{feedback}");
        }
    }

    #[test]
    fn input_coupled_position_state_in_snapshot_is_reactive_ok() {
        let source = r#"
import { useEffect, useRef, useState } from "react";
export default function Game() {
  const playerRef = useRef({ x: 0, y: 0 });
  const keysPressed = useRef(new Set<string>());
  const [playerX, setPlayerX] = useState(0);
  useEffect(() => {
    const down = (event: KeyboardEvent) => keysPressed.current.add(event.code);
    window.addEventListener('keydown', down);
    const tick = () => {
      if (keysPressed.current.has('ArrowLeft')) {
        playerRef.current.x -= 5;
        setPlayerX(playerRef.current.x);
      }
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }, []);
  return <main data-anvil-state={JSON.stringify({ playerX })} />;
}
"#;

        let diagnosis = diagnose_source_state_binding("src/app/page.tsx", source).unwrap();

        assert_eq!(
            diagnosis.diagnosis,
            StateBindingDiagnosisKind::StateReactiveOk
        );
    }

    #[test]
    fn static_app_without_input_does_not_emit_input_coupled_diagnosis() {
        let source = r#"
import { useState } from "react";
export default function Counter() {
  const [score, setScore] = useState(0);
  return <main data-anvil-state={JSON.stringify({ score })}>Score {score}</main>;
}
"#;

        let diagnosis = diagnose_source_state_binding("src/app/page.tsx", source).unwrap();

        assert_ne!(
            diagnosis.diagnosis,
            StateBindingDiagnosisKind::InputCoupledStateNotSnapshotted
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
        assert_eq!(
            diagnosis.undeterminable_reason,
            Some(UndeterminableReason::ComplexExpression)
        );
    }

    #[test]
    fn undeterminable_feedback_includes_neutral_facts_and_reason_event() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src/app");
        let events = dir.path().join("events.jsonl");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("page.tsx"),
            r#"
import { useState } from "react";
export default function Game() {
  const [items, setItems] = useState<Map<string, number>>(new Map());
  return <main data-anvil-state={JSON.stringify({ items })} />;
}
"#,
        )
        .unwrap();
        let report = VerificationReport::profile_failed(
            "browser_interaction_failed:input_state_change_missing_after_start",
        );

        let feedback = final_acceptance_feedback(dir.path(), "nextjs", &report, Some(&events));

        assert!(
            feedback.contains("State binding diagnosis: undeterminable"),
            "{feedback}"
        );
        assert!(feedback.contains("items declaration: line"), "{feedback}");
        assert!(
            feedback.contains("Minimum contract: after start and after input"),
            "{feedback}"
        );
        assert!(!feedback.contains("入力は"), "{feedback}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(
            event_text.contains(r#""undeterminable_reason":"generic_declaration_unmatched""#),
            "{event_text}"
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

    #[test]
    fn no_expression_feedback_includes_contract_attribute_guidance() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src/app");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("page.tsx"),
            r#"
export default function Game() {
  return <main><button data-anvil-action="primary">Start</button></main>;
}
"#,
        )
        .unwrap();
        let report = VerificationReport::profile_failed(
            "missing_required_evidence:stateful_update_evidence",
        );

        let feedback = final_acceptance_feedback(dir.path(), "nextjs", &report, None);

        assert!(feedback.contains("State binding diagnosis: undeterminable"));
        assert!(
            feedback.contains("Contract attribute repair guidance:"),
            "{feedback}"
        );
        assert!(
            feedback.contains("contract_attribute_missing"),
            "{feedback}"
        );
        assert!(
            feedback.contains("missing attribute: `data-anvil-state`"),
            "{feedback}"
        );
        assert!(
            feedback.contains("target source file: `src/app/page.tsx`"),
            "{feedback}"
        );
        assert!(
            feedback.contains("data-anvil-state={JSON.stringify({ phase, score, playerX })}"),
            "{feedback}"
        );
    }

    #[test]
    fn route_entry_is_diagnosed_before_layout_for_no_expression() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src/app");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("layout.tsx"),
            "export default function Layout({children}){ return <html><body>{children}</body></html>; }",
        )
        .unwrap();
        std::fs::write(
            src.join("page.tsx"),
            "export default function Page(){ return <main>Game</main>; }",
        )
        .unwrap();

        let diagnosis = diagnose_route_bound_state_binding(dir.path(), "nextjs");

        assert_eq!(
            diagnosis.diagnosis,
            StateBindingDiagnosisKind::Undeterminable
        );
        assert_eq!(
            diagnosis.undeterminable_reason,
            Some(UndeterminableReason::NoExpression)
        );
        assert_eq!(diagnosis.path, "src/app/page.tsx");
    }
}
