//! Narrow deterministic repair for unambiguous compiler diagnostics.
//!
//! This module deliberately handles only small local edits with high-signal
//! diagnostics. Ambiguous cases return `Skipped` and fall back to the normal
//! DiagnosticRepairWorker / verifier-repair pass.

use std::path::Path;

use super::Agent;
use super::repair_driver::VERIFIER_REPAIR_PASS_MAX_FILE_BYTES;
use super::repair_patch_validation::{
    RepairIntentEdit, ValidatedVerifierRepairEdit, detect_repair_candidate_weakening_patterns,
    repair_intent_edits_fingerprint, validate_repair_candidate_changed,
    validate_repair_candidate_contents, validate_repair_candidate_weakening_patterns,
    validate_repair_intent_not_replayed,
};
use super::task_contract::{RecoveryTargetHint, TaskKind};
use crate::logging::log_llm_event;
use crate::safety::path_guard::resolve_user_path;
use crate::session::feedback::mask_secrets;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MechanicalCompileRepairOutcome {
    Applied { relative_path: String },
    Skipped { reason: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MechanicalCompileRepairKind {
    RustMethodAsField,
    RustTrailingSemicolonReturn,
}

impl MechanicalCompileRepairKind {
    fn label(self) -> &'static str {
        match self {
            Self::RustMethodAsField => "rust_method_as_field",
            Self::RustTrailingSemicolonReturn => "rust_trailing_semicolon_return",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MechanicalCompileRepairCandidate {
    kind: MechanicalCompileRepairKind,
    updated_contents: String,
    fingerprint_old: String,
    fingerprint_new: String,
}

pub(super) fn try_apply_mechanical_compile_repair(
    agent: &mut Agent,
    target_hint: &RecoveryTargetHint,
) -> MechanicalCompileRepairOutcome {
    let Some(context) = agent.repair_job.clone() else {
        return MechanicalCompileRepairOutcome::Skipped {
            reason: "missing_repair_job",
        };
    };
    let diagnostic = mechanical_repair_diagnostic_for_job(&context);
    let Some((canonical_path, original_contents)) =
        read_repair_target(&agent.work_root, target_hint)
    else {
        return MechanicalCompileRepairOutcome::Skipped {
            reason: "target_unavailable",
        };
    };
    let Some(candidate) =
        mechanical_compile_repair_candidate(&target_hint.path, &diagnostic, &original_contents)
    else {
        return MechanicalCompileRepairOutcome::Skipped {
            reason: "no_candidate",
        };
    };
    let edit_payload = [RepairIntentEdit {
        old_string: &candidate.fingerprint_old,
        new_string: &candidate.fingerprint_new,
        replace_all: false,
    }];
    let fingerprint = repair_intent_edits_fingerprint(
        &context.failure_signature,
        &target_hint.path,
        &edit_payload,
    );
    if validate_mechanical_candidate(
        candidate.kind,
        &target_hint.path,
        &original_contents,
        &candidate.updated_contents,
        &context.applied_repair_intents,
        &fingerprint,
        repair_task_kind(agent),
    )
    .is_err()
    {
        log_mechanical_repair_skipped(agent, target_hint, candidate.kind, "validation_failed");
        return MechanicalCompileRepairOutcome::Skipped {
            reason: "validation_failed",
        };
    }
    let edit = ValidatedVerifierRepairEdit::new(
        target_hint.path.clone(),
        canonical_path,
        &original_contents,
        candidate.updated_contents,
        fingerprint,
    );
    let undo = match super::repair_patch_executor::apply_validated_repair_edit(&edit) {
        Ok(undo) => undo,
        Err(err) => {
            log_mechanical_repair_skipped(agent, target_hint, candidate.kind, "apply_failed");
            log_llm_event(
                "agent.verifier_mechanical_repair.apply_failed",
                serde_json::json!({
                    "session_id": agent.session_store.session_id(),
                    "path": target_hint.path,
                    "kind": candidate.kind.label(),
                    "error": mask_secrets(&err),
                }),
            );
            return MechanicalCompileRepairOutcome::Skipped {
                reason: "apply_failed",
            };
        }
    };
    super::verifier_orchestration::record_controller_verifier_repair_edit(
        agent,
        &edit.relative_path,
        &edit.fingerprint,
        target_hint,
        undo,
    );
    log_llm_event(
        "agent.verifier_mechanical_repair.applied",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "path": edit.relative_path,
            "kind": candidate.kind.label(),
            "preimage_hash": edit.preimage_hash,
            "postimage_hash": edit.postimage_hash,
        }),
    );
    MechanicalCompileRepairOutcome::Applied {
        relative_path: edit.relative_path,
    }
}

fn read_repair_target(
    work_root: &Path,
    target_hint: &RecoveryTargetHint,
) -> Option<(std::path::PathBuf, String)> {
    let path = resolve_user_path(work_root, &target_hint.path).ok()?;
    let canonical = std::fs::canonicalize(&path).ok()?;
    if !canonical.is_file() {
        return None;
    }
    let metadata = std::fs::metadata(&canonical).ok()?;
    if metadata.len() > VERIFIER_REPAIR_PASS_MAX_FILE_BYTES {
        return None;
    }
    let contents = std::fs::read_to_string(&canonical).ok()?;
    Some((canonical, contents))
}

fn validate_mechanical_candidate(
    kind: MechanicalCompileRepairKind,
    relative_path: &str,
    original_contents: &str,
    candidate_contents: &str,
    applied_repair_intents: &[String],
    fingerprint: &str,
    task_kind: TaskKind,
) -> Result<(), String> {
    validate_repair_candidate_changed(original_contents, candidate_contents)
        .map_err(|_| "mechanical repair produced no change".to_string())?;
    validate_repair_intent_not_replayed(applied_repair_intents, fingerprint)
        .map_err(|_| "mechanical repair was already applied".to_string())?;
    let weakening = detect_repair_candidate_weakening_patterns(
        relative_path,
        original_contents,
        candidate_contents,
    );
    if !weakening.patterns.is_empty()
        && !mechanical_assertions_preserved(kind, original_contents, candidate_contents)
    {
        validate_repair_candidate_weakening_patterns(weakening.patterns, weakening.rejection_kind)
            .map_err(|err| format!("mechanical repair rejected as weakening: {err:?}"))?;
    }
    validate_repair_candidate_contents(relative_path, candidate_contents, false, task_kind)
        .map_err(|err| format!("mechanical repair cheap check rejected: {err:?}"))?;
    Ok(())
}

fn mechanical_assertions_preserved(
    kind: MechanicalCompileRepairKind,
    original_contents: &str,
    candidate_contents: &str,
) -> bool {
    matches!(
        kind,
        MechanicalCompileRepairKind::RustMethodAsField
            | MechanicalCompileRepairKind::RustTrailingSemicolonReturn
    ) && assertion_token_count(candidate_contents) >= assertion_token_count(original_contents)
}

fn assertion_token_count(contents: &str) -> usize {
    ["assert!(", "assert_eq!(", "assert_ne!(", "debug_assert!("]
        .iter()
        .map(|needle| contents.matches(needle).count())
        .sum()
}

fn repair_task_kind(agent: &Agent) -> TaskKind {
    super::task_classification::task_contract_authority(agent)
        .map(|contract| contract.task_kind)
        .unwrap_or(TaskKind::Coding)
}

fn mechanical_repair_diagnostic_for_job(job: &super::repair_job::RepairJob) -> String {
    let error_kind = job
        .error_kind
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(job.failure_signature.as_str());
    if job.output_excerpt.trim().is_empty() {
        error_kind.to_string()
    } else {
        format!("{error_kind}\n{}", job.output_excerpt)
    }
}

fn mechanical_compile_repair_candidate(
    relative_path: &str,
    diagnostic: &str,
    contents: &str,
) -> Option<MechanicalCompileRepairCandidate> {
    if !relative_path.ends_with(".rs") {
        return None;
    }
    rust_method_as_field_candidate(diagnostic, contents)
        .or_else(|| rust_trailing_semicolon_return_candidate(diagnostic, contents))
}

fn rust_method_as_field_candidate(
    diagnostic: &str,
    contents: &str,
) -> Option<MechanicalCompileRepairCandidate> {
    let lower = diagnostic.to_ascii_lowercase();
    if !lower.contains("error[e0615]") || !lower.contains("use parentheses") {
        return None;
    }
    let method = extract_backtick_value_after(diagnostic, "method `")?;
    let occurrences = eligible_method_field_occurrences(contents, &method);
    let [start] = occurrences.as_slice() else {
        return None;
    };
    let end = start + 1 + method.len();
    let mut updated = String::with_capacity(contents.len() + 2);
    updated.push_str(&contents[..end]);
    updated.push_str("()");
    updated.push_str(&contents[end..]);
    Some(MechanicalCompileRepairCandidate {
        kind: MechanicalCompileRepairKind::RustMethodAsField,
        updated_contents: updated,
        fingerprint_old: format!(".{method}"),
        fingerprint_new: format!(".{method}()"),
    })
}

fn rust_trailing_semicolon_return_candidate(
    diagnostic: &str,
    contents: &str,
) -> Option<MechanicalCompileRepairCandidate> {
    let lower = diagnostic.to_ascii_lowercase();
    if !lower.contains("error[e0308]")
        || !lower.contains("mismatched types")
        || !lower.contains("found `()`")
    {
        return None;
    }
    let occurrences = eligible_return_semicolon_occurrences(contents);
    let [semicolon] = occurrences.as_slice() else {
        return None;
    };
    let mut updated = String::with_capacity(contents.len().saturating_sub(1));
    updated.push_str(&contents[..*semicolon]);
    updated.push_str(&contents[semicolon + 1..]);
    Some(MechanicalCompileRepairCandidate {
        kind: MechanicalCompileRepairKind::RustTrailingSemicolonReturn,
        updated_contents: updated,
        fingerprint_old: ";".to_string(),
        fingerprint_new: "<removed trailing return semicolon>".to_string(),
    })
}

fn extract_backtick_value_after(text: &str, prefix: &str) -> Option<String> {
    let start = text.find(prefix)? + prefix.len();
    let rest = &text[start..];
    let end = rest.find('`')?;
    let value = &rest[..end];
    (!value.is_empty()
        && value
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric()))
    .then(|| value.to_string())
}

fn eligible_method_field_occurrences(contents: &str, method: &str) -> Vec<usize> {
    let needle = format!(".{method}");
    let mut starts = Vec::new();
    let mut offset = 0usize;
    while let Some(relative) = contents[offset..].find(&needle) {
        let start = offset + relative;
        let after = start + needle.len();
        if method_access_suffix_is_eligible(contents[after..].chars().next()) {
            starts.push(start);
        }
        offset = after;
    }
    starts
}

fn method_access_suffix_is_eligible(next: Option<char>) -> bool {
    match next {
        Some('(') => false,
        Some(ch) if ch == '_' || ch.is_ascii_alphanumeric() => false,
        _ => true,
    }
}

fn eligible_return_semicolon_occurrences(contents: &str) -> Vec<usize> {
    let bytes = contents.as_bytes();
    let mut hits = Vec::new();
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] == b';' && semicolon_precedes_block_close(contents, idx) {
            let prefix = &contents[..idx];
            if let Some(open) = prefix.rfind('{') {
                let header = &prefix[..open];
                let header_start = header.rfind(['}', '\n']).map(|pos| pos + 1).unwrap_or(0);
                if header[header_start..].contains("->") {
                    hits.push(idx);
                }
            }
        }
        idx += 1;
    }
    hits
}

fn semicolon_precedes_block_close(contents: &str, semicolon: usize) -> bool {
    contents[semicolon + 1..]
        .chars()
        .find(|ch| !ch.is_whitespace())
        == Some('}')
}

fn log_mechanical_repair_skipped(
    agent: &Agent,
    target_hint: &RecoveryTargetHint,
    kind: MechanicalCompileRepairKind,
    reason: &'static str,
) {
    log_llm_event(
        "agent.verifier_mechanical_repair.skipped",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "path": target_hint.path,
            "kind": kind.label(),
            "reason": reason,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_run::commands::test_agent_with_config;
    use crate::config::Config;
    use crate::session::store::ConversationMessage;
    use tempfile::tempdir;

    #[test]
    fn rust_e0615_method_as_field_candidate_adds_parentheses_once() {
        let diagnostic = "error[E0615]: attempted to take value of method `status` on type `CommandOutput`\nhelp: use parentheses to call the method";
        let contents = "#[test]\nfn ndjson_status() {\n    assert!(output.status.success());\n}\n";

        let candidate =
            mechanical_compile_repair_candidate("tests/ndjson.rs", diagnostic, contents)
                .expect("unambiguous E0615 should produce candidate");

        assert_eq!(
            candidate.kind,
            MechanicalCompileRepairKind::RustMethodAsField
        );
        assert_eq!(
            candidate.updated_contents,
            "#[test]\nfn ndjson_status() {\n    assert!(output.status().success());\n}\n"
        );
    }

    #[test]
    fn rust_e0615_method_as_field_skips_ambiguous_occurrences() {
        let diagnostic = "error[E0615]: attempted to take value of method `status` on type `CommandOutput`\nhelp: use parentheses to call the method";
        let contents = "fn check() { assert!(a.status.success()); assert!(b.status.success()); }\n";

        assert!(
            mechanical_compile_repair_candidate("tests/ndjson.rs", diagnostic, contents).is_none()
        );
    }

    #[test]
    fn rust_e0308_return_type_mismatch_removes_single_trailing_semicolon() {
        let diagnostic = "error[E0308]: mismatched types\nexpected `String`, found `()`";
        let contents = "fn helper() -> String {\n    build_value();\n}\n";

        let candidate =
            mechanical_compile_repair_candidate("tests/helper.rs", diagnostic, contents)
                .expect("single trailing semicolon should be repairable");

        assert_eq!(
            candidate.updated_contents,
            "fn helper() -> String {\n    build_value()\n}\n"
        );
    }

    #[test]
    fn applying_mechanical_repair_records_patch_and_reruns_verifier_next() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        agent.session.messages.push(ConversationMessage::user(
            "Create a Rust NDJSON parser and verify it with cargo test.".to_string(),
        ));
        std::fs::create_dir_all(agent.work_root.join("tests")).unwrap();
        let test_path = agent.work_root.join("tests/ndjson.rs");
        std::fs::write(
            &test_path,
            "#[test]\nfn ndjson_status() {\n    assert!(output.status.success());\n}\n",
        )
        .unwrap();
        let target_hint = RecoveryTargetHint {
            role: super::super::task_contract::ArtifactRole::Test,
            path: "tests/ndjson.rs".to_string(),
            reason: "rust compiler points at method-as-field in generated test".to_string(),
        };
        let mut job = super::super::repair_job::RepairJob::new_for_test();
        job.command = "cargo test".to_string();
        job.output_excerpt = "error[E0615]: attempted to take value of method `status` on type `CommandOutput`\nhelp: use parentheses to call the method".to_string();
        job.failure_signature = "tests/ndjson.rs E0615 status".to_string();
        job.error_kind = Some("error[E0615]".to_string());
        job.target_hint = Some(target_hint.clone());
        job.repair_target_hint = Some(target_hint.clone());
        agent.repair_job = Some(job);

        let outcome = try_apply_mechanical_compile_repair(&mut agent, &target_hint);

        assert_eq!(
            outcome,
            MechanicalCompileRepairOutcome::Applied {
                relative_path: "tests/ndjson.rs".to_string()
            }
        );
        assert_eq!(
            std::fs::read_to_string(&test_path).unwrap(),
            "#[test]\nfn ndjson_status() {\n    assert!(output.status().success());\n}\n"
        );
        assert_eq!(
            agent.repair_job.as_ref().unwrap().next_action(),
            super::super::repair_job::RepairNextAction::RerunVerifier
        );
    }

    #[test]
    fn read_repair_target_skips_oversized_files() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("large.rs");
        std::fs::write(
            &target,
            "x".repeat(VERIFIER_REPAIR_PASS_MAX_FILE_BYTES as usize + 1),
        )
        .unwrap();
        let hint = RecoveryTargetHint {
            role: super::super::task_contract::ArtifactRole::Test,
            path: "large.rs".to_string(),
            reason: "too large".to_string(),
        };

        assert!(read_repair_target(temp.path(), &hint).is_none());
    }
}
