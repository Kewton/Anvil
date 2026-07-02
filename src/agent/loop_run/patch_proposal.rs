//! v0.4.13 Phase 5: bounded patch-shaper output schema.
//!
//! The patch shaper LLM does not choose the repair target. It returns a patch
//! proposal for the controller-selected target, and this module validates the
//! basic structural invariants before the existing apply path can consume it.

#![allow(dead_code)]

use super::repair_action::RepairAction;
use super::repair_brief::extract_last_json_object;
use super::repair_job::sanitize_repair_job_text_with_char_cap;

const MAX_EDITS: usize = 16;
const MAX_REASON_CHARS: usize = 240;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PatchProposal {
    pub(super) target_path: String,
    pub(super) edits: Vec<PatchEdit>,
    pub(super) explanation: String,
    pub(super) risk: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PatchEdit {
    pub(super) old_string: String,
    pub(super) new_string: String,
    pub(super) reason: String,
    pub(super) replace_all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PatchProposalError {
    ToolMarkup,
    JsonMissing,
    JsonMalformed,
    ObjectMissing,
    MissingField(&'static str),
    EditsEmpty,
    TooManyEdits,
    EditMalformed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PatchProposalRejection {
    TargetMismatch,
    EmptyOldString,
    OldStringNotFound,
    OldStringAmbiguous,
}

impl PatchProposalError {
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            Self::ToolMarkup => "tool_markup",
            Self::JsonMissing => "json_missing",
            Self::JsonMalformed => "json_malformed",
            Self::ObjectMissing => "object_missing",
            Self::MissingField(_) => "missing_field",
            Self::EditsEmpty => "edits_empty",
            Self::TooManyEdits => "too_many_edits",
            Self::EditMalformed => "edit_malformed",
        }
    }
}

impl PatchProposalRejection {
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            Self::TargetMismatch => "target_mismatch",
            Self::EmptyOldString => "empty_old_string",
            Self::OldStringNotFound => "old_string_not_found",
            Self::OldStringAmbiguous => "old_string_ambiguous",
        }
    }
}

pub(super) fn parse_patch_proposal_reply(reply: &str) -> Result<PatchProposal, PatchProposalError> {
    let lower = reply.to_ascii_lowercase();
    if lower.contains("<anvil_tool_call")
        || lower.contains("</anvil_tool_call>")
        || lower.contains("\"tool_calls\"")
        || lower.contains("\"tool_call\"")
    {
        return Err(PatchProposalError::ToolMarkup);
    }
    let json = extract_patch_proposal_json_object(reply)?;
    let value = parse_patch_proposal_json_value(&json)?;
    let object = value.as_object().ok_or(PatchProposalError::ObjectMissing)?;
    let target_path = object
        .get("target_path")
        .or_else(|| object.get("path"))
        .and_then(serde_json::Value::as_str)
        .map(sanitize_path)
        .ok_or(PatchProposalError::MissingField("target_path"))?;
    let edits = if let Some(edits) = object.get("edits").and_then(serde_json::Value::as_array) {
        if edits.is_empty() {
            return Err(PatchProposalError::EditsEmpty);
        }
        if edits.len() > MAX_EDITS {
            return Err(PatchProposalError::TooManyEdits);
        }
        edits
            .iter()
            .map(parse_patch_edit)
            .collect::<Result<Vec<_>, _>>()?
    } else if object.contains_key("old_string") || object.contains_key("new_string") {
        vec![parse_patch_edit(&serde_json::Value::Object(
            object.clone(),
        ))?]
    } else {
        return Err(PatchProposalError::MissingField("edits"));
    };
    let explanation = object
        .get("explanation")
        .or_else(|| object.get("reason"))
        .and_then(serde_json::Value::as_str)
        .map(sanitize_reason)
        .unwrap_or_default();
    let risk = object
        .get("risk")
        .and_then(serde_json::Value::as_str)
        .map(sanitize_reason)
        .unwrap_or_default();

    Ok(PatchProposal {
        target_path,
        edits,
        explanation,
        risk,
    })
}

fn extract_patch_proposal_json_object(reply: &str) -> Result<String, PatchProposalError> {
    if let Some(json) = extract_last_json_object(reply) {
        return Ok(json.to_string());
    }
    repair_schema_key_quotes(reply)
        .and_then(|repaired| extract_last_json_object(&repaired).map(str::to_string))
        .ok_or(PatchProposalError::JsonMissing)
}

fn parse_patch_proposal_json_value(json: &str) -> Result<serde_json::Value, PatchProposalError> {
    if let Ok(value) = serde_json::from_str(json) {
        return Ok(value);
    }
    let mut candidates = Vec::new();
    if let Some(repaired) = repair_schema_key_quotes(json) {
        if let Some(second_pass) = repair_surplus_array_closers(&repaired) {
            candidates.push(second_pass);
        }
        candidates.push(repaired);
    }
    if let Some(repaired) = repair_surplus_array_closers(json) {
        candidates.push(repaired);
    }
    candidates
        .iter()
        .find_map(|candidate| serde_json::from_str(candidate).ok())
        .ok_or(PatchProposalError::JsonMalformed)
}

fn repair_schema_key_quotes(raw: &str) -> Option<String> {
    const KEYS: &[&str] = &[
        "target_path",
        "path",
        "edits",
        "old_string",
        "new_string",
        "replace_all",
        "reason",
        "explanation",
        "risk",
    ];
    let mut out = String::with_capacity(raw.len() + 8);
    let mut idx = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut previous_significant: Option<char> = None;
    let mut changed = false;

    while idx < raw.len() {
        let ch = raw[idx..].chars().next()?;
        if in_string {
            out.push(ch);
            idx += ch.len_utf8();
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if previous_significant.is_some_and(|prev| prev == '{' || prev == ',') {
            let mut whitespace_end = idx;
            while whitespace_end < raw.len() {
                let ws = raw[whitespace_end..].chars().next()?;
                if !ws.is_whitespace() {
                    break;
                }
                out.push(ws);
                whitespace_end += ws.len_utf8();
            }
            if whitespace_end != idx {
                idx = whitespace_end;
                if idx >= raw.len() {
                    break;
                }
            }
            if let Some((key, delimiter_len)) = schema_key_quote_repair(raw, idx, KEYS) {
                out.push('"');
                out.push_str(key);
                out.push_str("\":");
                idx += key.len() + delimiter_len;
                previous_significant = Some(':');
                changed = true;
                continue;
            }
        }

        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
                previous_significant = Some(ch);
            }
            _ => {
                out.push(ch);
                if !ch.is_whitespace() {
                    previous_significant = Some(ch);
                }
            }
        }
        idx += ch.len_utf8();
    }

    changed.then_some(out)
}

fn schema_key_quote_repair<'a>(
    raw: &'a str,
    idx: usize,
    keys: &'a [&'a str],
) -> Option<(&'a str, usize)> {
    keys.iter().copied().find_map(|key| {
        let rest = raw.get(idx..)?;
        if !rest.starts_with(key) {
            return None;
        }
        let after_key = idx + key.len();
        let after = raw.get(after_key..)?;
        if after.starts_with("\":") {
            Some((key, 2))
        } else if after.starts_with(':') {
            Some((key, 1))
        } else {
            None
        }
    })
}

fn repair_surplus_array_closers(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut stack = Vec::<char>::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut changed = false;

    for ch in raw.chars() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            '{' | '[' => {
                stack.push(ch);
                out.push(ch);
            }
            '}' => {
                if stack.last() == Some(&'{') {
                    stack.pop();
                }
                out.push(ch);
            }
            ']' => {
                if stack.last() == Some(&'[') {
                    stack.pop();
                    out.push(ch);
                } else {
                    changed = true;
                }
            }
            _ => out.push(ch),
        }
    }

    changed.then_some(out)
}

pub(super) fn validate_patch_proposal_for_action(
    proposal: &PatchProposal,
    action: &RepairAction,
    target_contents: &str,
) -> Result<(), PatchProposalRejection> {
    if proposal.target_path != action.target_path {
        return Err(PatchProposalRejection::TargetMismatch);
    }
    let mut simulated = target_contents.to_string();
    for edit in &proposal.edits {
        if edit.old_string.is_empty() {
            return Err(PatchProposalRejection::EmptyOldString);
        }
        let count = simulated.matches(&edit.old_string).count();
        if edit.replace_all {
            if count == 0 {
                return Err(PatchProposalRejection::OldStringNotFound);
            }
            simulated = simulated.replace(&edit.old_string, &edit.new_string);
        } else {
            match count {
                0 => return Err(PatchProposalRejection::OldStringNotFound),
                1 => {
                    simulated = simulated.replacen(&edit.old_string, &edit.new_string, 1);
                }
                _ => return Err(PatchProposalRejection::OldStringAmbiguous),
            }
        }
    }
    Ok(())
}

fn parse_patch_edit(value: &serde_json::Value) -> Result<PatchEdit, PatchProposalError> {
    let object = value.as_object().ok_or(PatchProposalError::EditMalformed)?;
    let old_string = object
        .get("old_string")
        .or_else(|| object.get("old"))
        .and_then(serde_json::Value::as_str)
        .ok_or(PatchProposalError::MissingField("old_string"))?;
    let new_string = object
        .get("new_string")
        .or_else(|| object.get("new"))
        .and_then(serde_json::Value::as_str)
        .ok_or(PatchProposalError::MissingField("new_string"))?;
    let reason = object
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .map(sanitize_reason)
        .unwrap_or_default();
    let replace_all = object
        .get("replace_all")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Ok(PatchEdit {
        old_string: old_string.to_string(),
        new_string: new_string.to_string(),
        reason,
        replace_all,
    })
}

fn sanitize_path(input: &str) -> String {
    sanitize_repair_job_text_with_char_cap(input, MAX_REASON_CHARS)
        .replace('\\', "/")
        .trim()
        .trim_start_matches("./")
        .to_string()
}

fn sanitize_reason(input: &str) -> String {
    sanitize_repair_job_text_with_char_cap(input, MAX_REASON_CHARS)
}

#[cfg(test)]
mod tests {
    use super::super::repair_action::RepairAction;
    use super::super::repair_brief::{AllowedChangeKind, SourceOfTruth};
    use super::super::task_contract::ArtifactRole;
    use super::*;

    fn action() -> RepairAction {
        RepairAction {
            target_role: ArtifactRole::Implementation,
            target_path: "app/main.py".to_string(),
            allowed_change_kind: AllowedChangeKind::FixImplementationBehavior,
            source_of_truth: SourceOfTruth::UserRequest,
            budget: 2,
            brief_confidence: 0.9,
        }
    }

    #[test]
    fn patch_proposal_extracts_json_after_prose() {
        let reply = r#"Thinking...
{"target_path":"./app\\main.py","edits":[{"old_string":"return 201","new_string":"return 200"}],"explanation":"fix status","risk":"low"}"#;

        let proposal = parse_patch_proposal_reply(reply).unwrap();

        assert_eq!(proposal.target_path, "app/main.py");
        assert_eq!(proposal.edits[0].old_string, "return 201");
        assert_eq!(proposal.edits[0].new_string, "return 200");
    }

    #[test]
    fn patch_proposal_extracts_json_with_braces_inside_strings() {
        let reply = r#"{
  "target_path": "tests/test_main.py",
  "edits": [
    {
      "old_string": "response = client.delete(f\"/items/{item_id}\")\nassert response.status_code == 200",
      "new_string": "response = client.delete(f\"/items/{item_id}\")\nassert response.status_code == 204",
      "replace_all": false,
      "reason": "align DELETE /items/{id} expectation"
    }
  ],
  "explanation": "fix generated expectation",
  "risk": "low"
}"#;

        let proposal = parse_patch_proposal_reply(reply).unwrap();

        assert_eq!(proposal.target_path, "tests/test_main.py");
        assert!(proposal.edits[0].old_string.contains("{item_id}"));
        assert!(proposal.edits[0].reason.contains("{id}"));
    }

    #[test]
    fn patch_proposal_rejects_tool_markup() {
        let reply = r#"<anvil_tool_call>{"target_path":"app/main.py"}</anvil_tool_call>"#;

        assert_eq!(
            parse_patch_proposal_reply(reply),
            Err(PatchProposalError::ToolMarkup)
        );
    }

    #[test]
    fn patch_proposal_validates_exact_once_sequence() {
        let proposal = PatchProposal {
            target_path: "app/main.py".to_string(),
            edits: vec![PatchEdit {
                old_string: "return 201".to_string(),
                new_string: "return 200".to_string(),
                reason: String::new(),
                replace_all: false,
            }],
            explanation: String::new(),
            risk: String::new(),
        };

        assert_eq!(
            validate_patch_proposal_for_action(
                &proposal,
                &action(),
                "def create():\n    return 201\n"
            ),
            Ok(())
        );
    }

    #[test]
    fn patch_proposal_rejects_ambiguous_old_string() {
        let proposal = PatchProposal {
            target_path: "app/main.py".to_string(),
            edits: vec![PatchEdit {
                old_string: "return 201".to_string(),
                new_string: "return 200".to_string(),
                reason: String::new(),
                replace_all: false,
            }],
            explanation: String::new(),
            risk: String::new(),
        };

        assert_eq!(
            validate_patch_proposal_for_action(&proposal, &action(), "return 201\nreturn 201\n"),
            Err(PatchProposalRejection::OldStringAmbiguous)
        );
    }

    #[test]
    fn patch_proposal_accepts_single_edit_schema() {
        let reply = r#"{"path":"app/main.py","old_string":"old","new_string":"new","reason":"fix","replace_all":true}"#;

        let proposal = parse_patch_proposal_reply(reply).unwrap();

        assert_eq!(proposal.target_path, "app/main.py");
        assert_eq!(proposal.edits.len(), 1);
        assert_eq!(proposal.edits[0].old_string, "old");
        assert_eq!(proposal.edits[0].new_string, "new");
        assert_eq!(proposal.edits[0].reason, "fix");
        assert!(proposal.edits[0].replace_all);
    }

    #[test]
    fn patch_proposal_repairs_unquoted_schema_key() {
        let reply = r#"{"path":"src/lib.rs","old_string":"fn value() -> i32 { 1 }",new_string":"pub fn value() -> i32 { 1 }","reason":"export function"}"#;

        let proposal = parse_patch_proposal_reply(reply).unwrap();

        assert_eq!(proposal.target_path, "src/lib.rs");
        assert_eq!(proposal.edits[0].old_string, "fn value() -> i32 { 1 }");
        assert_eq!(proposal.edits[0].new_string, "pub fn value() -> i32 { 1 }");
    }

    #[test]
    fn patch_proposal_repairs_surplus_array_closer() {
        let reply = r#"{"path":"src/lib.rs","edits":[{"old_string":"fn value() -> i32 { 1 }","new_string":"pub fn value() -> i32 { 1 }","reason":"export function"}]],"reason":"fix"}"#;

        let proposal = parse_patch_proposal_reply(reply).unwrap();

        assert_eq!(proposal.target_path, "src/lib.rs");
        assert_eq!(proposal.edits.len(), 1);
        assert_eq!(proposal.edits[0].new_string, "pub fn value() -> i32 { 1 }");
    }

    #[test]
    fn patch_proposal_repairs_combined_schema_drift() {
        let reply = r#"{"path":"src/lib.rs","edits":[{"old_string":"fn value() -> i32 { 1 }",new_string":"pub fn value() -> i32 { 1 }","reason":"export function"}]],"reason":"fix"}"#;

        let proposal = parse_patch_proposal_reply(reply).unwrap();

        assert_eq!(proposal.target_path, "src/lib.rs");
        assert_eq!(proposal.edits[0].new_string, "pub fn value() -> i32 { 1 }");
    }
}
