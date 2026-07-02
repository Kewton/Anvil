//! Controller state packet parsing for TaskContract input projection.
//!
//! This module owns the structural `STATE_CONTROL_PACKET` view of a request.
//! It converts controller-authored JSON into typed artifact obligations and a
//! model-visible natural-language request without letting schema keys leak into
//! task or tool policy inference.

use super::task_contract::{ArtifactObligation, ArtifactRole, TaskKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ControllerStatePacket {
    required_artifacts: Vec<ArtifactObligation>,
    evidence_command: Option<String>,
}

impl ControllerStatePacket {
    fn from_value(value: &serde_json::Value) -> Self {
        let required_artifacts = value
            .get("required_artifacts")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(controller_artifact_obligation)
            .collect::<Vec<_>>();
        let evidence_command = value
            .get("evidence_command")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .map(str::to_string);
        Self {
            required_artifacts,
            evidence_command,
        }
    }

    pub(super) fn evidence_command(&self) -> Option<&str> {
        self.evidence_command.as_deref()
    }

    pub(super) fn inferred_task_kind(&self) -> Option<TaskKind> {
        if self.required_artifacts.is_empty() {
            return None;
        }
        if self
            .required_artifacts
            .iter()
            .any(|artifact| artifact.role == ArtifactRole::DataOutput)
        {
            Some(TaskKind::Data)
        } else if self
            .required_artifacts
            .iter()
            .all(|artifact| artifact.role == ArtifactRole::UsageDocs)
        {
            Some(TaskKind::Docs)
        } else {
            Some(TaskKind::Coding)
        }
    }

    pub(super) fn extend_contract_parts(
        &self,
        required: &mut Vec<ArtifactRole>,
        required_artifact_identities: &mut Vec<ArtifactObligation>,
    ) {
        for identity in &self.required_artifacts {
            if !required.contains(&identity.role) {
                required.push(identity.role);
            }
            super::task_contract::push_or_merge_artifact_obligation(
                required_artifact_identities,
                identity.clone(),
            );
        }
    }
}

/// Single parsed view of a raw request used by controller/state code.
///
/// The controller packet is structural state. Natural-language inference,
/// WorkMode classification, and model-visible prompt history must use
/// `visible_text`, not the raw request, so schema keys cannot leak into task or
/// tool policy decisions.
#[derive(Debug, Clone)]
pub(super) struct RequestInferenceView {
    visible_text: String,
    pub(super) controller_state: Option<ControllerStatePacket>,
    controller_packet_at_start: bool,
}

impl RequestInferenceView {
    pub(super) fn from_raw(raw: &str) -> Self {
        let parsed_packet = controller_state_packet_value_and_range(raw);
        let (controller_state, visible_text) = match parsed_packet {
            Some((value, range)) => (
                Some(ControllerStatePacket::from_value(&value)),
                model_visible_request_text_from_packet_range(raw, range),
            ),
            None => (None, model_visible_request_text_without_packet(raw)),
        };
        let controller_packet_at_start = raw.trim_start().starts_with("STATE_CONTROL_PACKET");
        Self {
            visible_text,
            controller_state,
            controller_packet_at_start,
        }
    }

    pub(super) fn visible_text(&self) -> &str {
        &self.visible_text
    }

    pub(super) fn into_visible_text(self) -> String {
        self.visible_text
    }

    pub(super) fn is_controller_owned_turn(&self) -> bool {
        self.controller_packet_at_start
    }
}

fn controller_state_packet_value_and_range(
    raw: &str,
) -> Option<(serde_json::Value, std::ops::Range<usize>)> {
    let marker_start = raw.find("STATE_CONTROL_PACKET")?;
    let json_start = marker_start + raw[marker_start..].find('{')?;
    let tail = &raw[json_start..];
    let mut stream = serde_json::Deserializer::from_str(tail).into_iter::<serde_json::Value>();
    let value = stream.next()?.ok()?;
    let json_end = json_start + stream.byte_offset();
    Some((value, marker_start..json_end))
}

pub(super) fn model_visible_request_text(raw: &str) -> String {
    RequestInferenceView::from_raw(raw).into_visible_text()
}

fn model_visible_request_text_without_packet(raw: &str) -> String {
    if let Some(marker_start) = raw.find("STATE_CONTROL_PACKET") {
        return raw[..marker_start].trim_end().to_string();
    }
    raw.trim().to_string()
}

fn model_visible_request_text_from_packet_range(
    raw: &str,
    range: std::ops::Range<usize>,
) -> String {
    let before = raw[..range.start].trim_end();
    let after = raw[range.end..].trim_start_matches(|ch: char| {
        ch.is_whitespace() || matches!(ch, '.' | '。' | ',' | '、' | ';' | '；')
    });
    match (before.is_empty(), after.is_empty()) {
        (true, true) => String::new(),
        (false, true) => before.to_string(),
        (true, false) => after.to_string(),
        (false, false) => format!("{before} {after}"),
    }
}

fn controller_artifact_obligation(value: &serde_json::Value) -> Option<ArtifactObligation> {
    let path = value
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())?;
    let role = value
        .get("role")
        .and_then(serde_json::Value::as_str)
        .and_then(controller_artifact_role_from_label)
        .or_else(|| {
            let category =
                super::completion_evidence::classify_repo_edit_path(std::path::Path::new(path));
            super::task_contract::role_from_repo_edit(category)
        })?;
    let data_fields = controller_schema_labels(
        value,
        &["columns", "json_fields", "fields", "schema_fields"],
    );
    let required_sections =
        controller_schema_labels(value, &["required_sections", "sections", "schema_sections"]);
    let obligation = match role {
        ArtifactRole::DataOutput if !data_fields.is_empty() => {
            ArtifactObligation::structured_record(path, data_fields)
        }
        ArtifactRole::UsageDocs if !required_sections.is_empty() => {
            ArtifactObligation::readme(path, required_sections)
        }
        _ => ArtifactObligation::file(role, path),
    };
    Some(obligation)
}

fn controller_artifact_role_from_label(raw: &str) -> Option<ArtifactRole> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "source" | "implementation" | "impl" | "code" => Some(ArtifactRole::Implementation),
        "test" | "tests" | "verifier" => Some(ArtifactRole::Test),
        "manifest" | "setup" | "config" | "package_manifest" => Some(ArtifactRole::Setup),
        "document" | "docs" | "usage_docs" | "runbook" | "research_notes" | "prose" => {
            Some(ArtifactRole::UsageDocs)
        }
        "output_file" | "data_output" | "data" | "json" | "csv" => Some(ArtifactRole::DataOutput),
        _ => None,
    }
}

const MAX_CONTROLLER_SCHEMA_LABELS: usize = 32;

fn controller_schema_labels(value: &serde_json::Value, keys: &[&str]) -> Vec<String> {
    for key in keys {
        let labels = controller_schema_labels_from_value(value.get(*key));
        if !labels.is_empty() {
            return labels;
        }
    }
    if let Some(schema) = value.get("schema") {
        for key in keys {
            let labels = controller_schema_labels_from_value(schema.get(*key));
            if !labels.is_empty() {
                return labels;
            }
        }
    }
    Vec::new()
}

fn controller_schema_labels_from_value(value: Option<&serde_json::Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let raw = match value {
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>(),
        serde_json::Value::String(s) => s.split([',', '|']).map(str::to_string).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let mut labels = Vec::new();
    for label in raw.into_iter().filter_map(controller_schema_label) {
        if !labels.contains(&label) {
            labels.push(label);
        }
        if labels.len() >= MAX_CONTROLLER_SCHEMA_LABELS {
            break;
        }
    }
    labels
}

fn controller_schema_label(raw: String) -> Option<String> {
    let normalized = raw
        .trim()
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect::<String>();
    let normalized = normalized.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    Some(super::task_contract::mask_and_cap_recovery_field(
        &normalized,
    ))
}
