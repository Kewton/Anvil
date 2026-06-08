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
