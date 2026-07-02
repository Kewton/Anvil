//! Display and prompt-safety projections for task contracts.
//!
//! This module owns masking and bounded rendering for contract obligations.
//! It has no authority over contract admission or completion state.

use super::task_contract::{ArtifactObligation, DeliverableSchema};

/// Issue #918 (P1): display cap (chars) for a single section/schema label.
/// Applied ONLY to display projections — never to stored contract values.
pub(super) const MAX_SECTION_LABEL_LEN: usize = 256;
/// Issue #918 (P1): display cap (count) on acceptance criteria. Applied only
/// at display projection time.
pub(super) const MAX_ACCEPTANCE_CRITERIA: usize = 32;

/// Per-value `mask_secrets` for a free-text obligation field.
pub(super) fn mask_obligation_value(value: &str) -> String {
    crate::session::feedback::mask_secrets(value)
}

/// Mask then char-boundary-cap a section/schema label for display.
pub(super) fn mask_and_cap_label(value: &str) -> String {
    let masked = mask_obligation_value(value);
    if masked.chars().count() <= MAX_SECTION_LABEL_LEN {
        masked
    } else {
        masked.chars().take(MAX_SECTION_LABEL_LEN).collect()
    }
}

/// Issue #918 (P1) follow-up (PR #930 review): SSOT mask+cap for any
/// obligation/hint-derived free-text rendered into a recovery prompt.
///
/// Recovery notes render `RecoveryTargetHint` path/reason directly into the LLM
/// request body, a path that does not pass through serde payload masking. Apply
/// masking at the render point.
pub(super) fn mask_and_cap_recovery_field(value: &str) -> String {
    mask_and_cap_label(value)
}

pub(super) fn join_masked_labels(values: &[String]) -> String {
    values
        .iter()
        .map(|v| mask_and_cap_label(v))
        .collect::<Vec<_>>()
        .join("|")
}

pub(super) fn obligation_report_label(obligation: &ArtifactObligation) -> String {
    let mut parts = vec![format!(
        "role={}, kind={}, path={}",
        obligation.role.label(),
        obligation.kind.label(),
        mask_obligation_value(&obligation.path)
    )];
    if !obligation.required_sections.is_empty() {
        parts.push(format!(
            "required_sections={}",
            join_masked_labels(&obligation.required_sections)
        ));
    }
    if !obligation.acceptance_criteria.is_empty() {
        let shown = obligation
            .acceptance_criteria
            .iter()
            .take(MAX_ACCEPTANCE_CRITERIA)
            .map(|c| mask_and_cap_label(c))
            .collect::<Vec<_>>()
            .join("|");
        parts.push(format!("acceptance_criteria={shown}"));
    }
    if let Some(DeliverableSchema::JsonFields(fields)) = obligation.schema.as_ref()
        && !fields.is_empty()
    {
        parts.push(format!("schema_fields={}", join_masked_labels(fields)));
    }
    if let Some(DeliverableSchema::StructuredRecord(schema)) = obligation.schema.as_ref()
        && !schema.columns.is_empty()
    {
        parts.push(format!(
            "schema_columns={}",
            join_masked_labels(&schema.columns)
        ));
        if !schema.expected_rows.is_empty() {
            let rows = schema
                .expected_rows
                .iter()
                .map(|row| join_masked_labels(row))
                .collect::<Vec<_>>()
                .join(";");
            parts.push(format!("schema_rows={rows}"));
        }
    }
    if let Some(DeliverableSchema::RequiredSections(sections)) = obligation.schema.as_ref()
        && !sections.is_empty()
    {
        parts.push(format!("schema_sections={}", join_masked_labels(sections)));
    }
    parts.join(", ")
}
