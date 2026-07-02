//! Verifier-output artifact candidate extraction.
//!
//! This module keeps structural path extraction out of diagnostic payload
//! assembly. It only admits existing, safe workspace artifacts; semantic target
//! choice stays with the diagnostic / admission pipeline.

use std::collections::HashSet;
use std::path::Path;

use super::task_contract::RecoveryTargetHint;
use super::verifier_repair_targeting::{
    extract_path_like_tokens, recovery_target_hint_for_existing_path,
};
use crate::util::workspace_paths::is_workspace_artifact_admitted_display_path;

const MAX_VERIFIER_OUTPUT_FAILURE_HINTS: usize = 12;

pub(super) const VERIFIER_OUTPUT_FAILURE_ARTIFACT_REASON: &str =
    "verifier output names this failure artifact";

pub(super) fn verifier_output_failure_hints(
    work_root: &Path,
    output_excerpt: &str,
) -> Vec<RecoveryTargetHint> {
    let mut hints = Vec::new();
    let mut seen = HashSet::new();
    for raw_path in extract_path_like_tokens(output_excerpt).take(MAX_VERIFIER_OUTPUT_FAILURE_HINTS)
    {
        let raw_path = verifier_output_candidate_path(work_root, raw_path);
        let Some(raw_path) = raw_path.as_deref() else {
            continue;
        };
        let Some(hint) = recovery_target_hint_for_existing_path(
            work_root,
            raw_path,
            VERIFIER_OUTPUT_FAILURE_ARTIFACT_REASON,
        ) else {
            continue;
        };
        if seen.insert(hint.path.clone()) {
            hints.push(hint);
        }
    }
    hints
}

fn verifier_output_candidate_path(work_root: &Path, raw_path: &str) -> Option<String> {
    let path = raw_path
        .trim()
        .trim_matches(|ch: char| matches!(ch, '(' | ')' | ',' | ';' | '"' | '\'' | '`'));
    if path.is_empty() || path.contains('\0') {
        return None;
    }
    let candidate = Path::new(path);
    if !candidate.is_absolute() {
        return Some(path.replace('\\', "/"));
    }

    let root = std::fs::canonicalize(work_root).ok()?;
    let canonical = std::fs::canonicalize(candidate).ok()?;
    let relative = canonical.strip_prefix(root).ok()?;
    let relative = relative.to_string_lossy().replace('\\', "/");
    if is_workspace_artifact_admitted_display_path(&relative) {
        Some(relative)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_run::task_contract::ArtifactRole;

    #[test]
    fn verifier_output_absolute_workspace_path_becomes_candidate_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("tests")).unwrap();
        let test_path = work_root.join("tests/test_text_rank.py");
        std::fs::write(&test_path, "def test_first(): pass\n").unwrap();
        let output = format!(
            "  File \"{}\", line 19, in test_first\nAssertionError: 'banana' != 'apple'",
            test_path.display()
        );

        let hints = verifier_output_failure_hints(work_root, &output);

        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].path, "tests/test_text_rank.py");
        assert_eq!(hints[0].role, ArtifactRole::Test);
    }

    #[test]
    fn verifier_output_absolute_path_outside_workspace_is_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_path = outside.path().join("tests/test_text_rank.py");
        std::fs::create_dir_all(outside_path.parent().unwrap()).unwrap();
        std::fs::write(&outside_path, "def test_first(): pass\n").unwrap();
        let output = format!("File \"{}\", line 1", outside_path.display());

        let hints = verifier_output_failure_hints(temp.path(), &output);

        assert!(hints.is_empty());
    }
}
