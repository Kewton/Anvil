//! Verifier evidence-scope packet construction.
//!
//! Evidence scope is controller-computed metadata for the diagnostic LLM. It
//! describes verifier coverage and known failure locations without deciding the
//! final repair target.

use std::path::Path;

use super::completion_evidence::is_completion_verifier_command;
use super::repair_job::RepairJob;
use super::task_contract::ArtifactRole;
use super::verifier_failure_artifacts::verifier_output_failure_hints;
use crate::session::feedback::mask_secrets;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VerifierEvidenceScopeKind {
    ProjectSuite,
    ArtifactFiltered,
    Unknown,
}

impl VerifierEvidenceScopeKind {
    fn label(self) -> &'static str {
        match self {
            VerifierEvidenceScopeKind::ProjectSuite => "project_suite",
            VerifierEvidenceScopeKind::ArtifactFiltered => "artifact_filtered",
            VerifierEvidenceScopeKind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VerifierEvidenceScopePacket {
    pub(super) kind: VerifierEvidenceScopeKind,
    pub(super) completion_verifier_command: bool,
    pub(super) command_references_changed_candidate: bool,
    pub(super) changed_candidate_count: usize,
    pub(super) changed_test_candidate_count: usize,
    pub(super) current_repair_target_path: Option<String>,
    pub(super) current_repair_target_role: Option<ArtifactRole>,
    pub(super) failure_location_path: Option<String>,
    pub(super) failure_location_role: Option<ArtifactRole>,
    pub(super) failure_location_differs_from_current_target: bool,
    pub(super) post_repair_rerun: bool,
}

impl VerifierEvidenceScopePacket {
    pub(super) fn to_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind.label(),
            "completion_verifier_command": self.completion_verifier_command,
            "command_references_changed_candidate": self.command_references_changed_candidate,
            "changed_candidate_count": self.changed_candidate_count,
            "changed_test_candidate_count": self.changed_test_candidate_count,
            "current_repair_target_path": self.current_repair_target_path.as_deref().map(mask_secrets),
            "current_repair_target_role": self.current_repair_target_role.map(ArtifactRole::label),
            "failure_location_path": self.failure_location_path.as_deref().map(mask_secrets),
            "failure_location_role": self.failure_location_role.map(ArtifactRole::label),
            "failure_location_differs_from_current_target": self.failure_location_differs_from_current_target,
            "post_repair_rerun": self.post_repair_rerun,
            "project_suite_failure_blocks_completion": self.kind == VerifierEvidenceScopeKind::ProjectSuite,
        })
    }
}

pub(super) fn verifier_evidence_scope_packet_for_context(
    work_root: &Path,
    context: &RepairJob,
) -> VerifierEvidenceScopePacket {
    let completion_verifier_command = is_completion_verifier_command(&context.command);
    let command_references_changed_candidate = context
        .changed_file_hints
        .iter()
        .any(|hint| command_references_workspace_path(&context.command, &hint.path));
    let changed_candidate_count = context.changed_file_hints.len();
    let changed_test_candidate_count = context
        .changed_file_hints
        .iter()
        .filter(|hint| hint.role == ArtifactRole::Test)
        .count();
    let output_failure_hint = verifier_output_failure_hints(work_root, &context.output_excerpt)
        .into_iter()
        .next();
    let current_repair_target_path = context
        .repair_target_hint
        .as_ref()
        .or(context.target_hint.as_ref())
        .map(|hint| hint.path.clone());
    let current_repair_target_role = context
        .repair_target_hint
        .as_ref()
        .or(context.target_hint.as_ref())
        .map(|hint| hint.role);
    let failure_location_path = output_failure_hint
        .as_ref()
        .or(context.target_hint.as_ref())
        .map(|hint| hint.path.clone());
    let failure_location_role = output_failure_hint
        .as_ref()
        .or(context.target_hint.as_ref())
        .map(|hint| hint.role);
    let failure_location_differs_from_current_target = failure_location_path
        .as_ref()
        .zip(current_repair_target_path.as_ref())
        .is_some_and(|(failure, current)| failure != current);
    let kind = if completion_verifier_command && !command_references_changed_candidate {
        VerifierEvidenceScopeKind::ProjectSuite
    } else if command_references_changed_candidate {
        VerifierEvidenceScopeKind::ArtifactFiltered
    } else {
        VerifierEvidenceScopeKind::Unknown
    };
    VerifierEvidenceScopePacket {
        kind,
        completion_verifier_command,
        command_references_changed_candidate,
        changed_candidate_count,
        changed_test_candidate_count,
        current_repair_target_path,
        current_repair_target_role,
        failure_location_path,
        failure_location_role,
        failure_location_differs_from_current_target,
        post_repair_rerun: context.rerun_outcome.is_some(),
    }
}

fn command_references_workspace_path(command: &str, path: &str) -> bool {
    let path = path.trim();
    if path.is_empty() {
        return false;
    }
    command
        .split(|ch: char| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    '\'' | '"' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
                )
        })
        .map(|token| {
            token.trim_matches(|ch: char| {
                matches!(
                    ch,
                    '\'' | '"' | '`' | ':' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}'
                )
            })
        })
        .any(|token| token == path)
}
