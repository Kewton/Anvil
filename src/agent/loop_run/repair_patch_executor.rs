//! Execution boundary for already validated verifier-repair patches.
//!
//! This module intentionally does not choose targets, validate authority, or
//! inspect LLM output. It only applies a `ValidatedVerifierRepairEdit` after
//! confirming the target preimage is unchanged.

use super::repair_patch_validation::ValidatedVerifierRepairEdit;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AppliedRepairPatchUndo {
    pub(super) relative_path: String,
    pub(super) canonical_path: std::path::PathBuf,
    pub(super) preimage_contents: String,
    pub(super) preimage_hash: String,
    pub(super) postimage_hash: String,
}

pub(super) fn apply_validated_repair_edit(
    edit: &ValidatedVerifierRepairEdit,
) -> Result<AppliedRepairPatchUndo, String> {
    let current = std::fs::read(&edit.canonical_path)
        .map_err(|err| format!("failed to read current target before apply: {err}"))?;
    let current_hash = sha256_hex(&current);
    if current_hash != edit.preimage_hash {
        return Err("preimage changed after validation".to_string());
    }
    let preimage_contents = String::from_utf8(current)
        .map_err(|_| "current repair target is not valid UTF-8".to_string())?;
    std::fs::write(&edit.canonical_path, edit.updated_contents.as_bytes())
        .map_err(|err| format!("failed to write validated repair target: {err}"))?;
    Ok(AppliedRepairPatchUndo {
        relative_path: edit.relative_path.clone(),
        canonical_path: edit.canonical_path.clone(),
        preimage_contents,
        preimage_hash: edit.preimage_hash.clone(),
        postimage_hash: edit.postimage_hash.clone(),
    })
}

pub(super) fn revert_applied_repair_patch(undo: &AppliedRepairPatchUndo) -> Result<(), String> {
    let current = std::fs::read(&undo.canonical_path)
        .map_err(|err| format!("failed to read current target before revert: {err}"))?;
    let current_hash = sha256_hex(&current);
    if current_hash != undo.postimage_hash {
        return Err("postimage changed after repair apply".to_string());
    }
    std::fs::write(&undo.canonical_path, undo.preimage_contents.as_bytes())
        .map_err(|err| format!("failed to restore repair preimage: {err}"))?;
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn applies_when_preimage_matches() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("main.py");
        std::fs::write(&target, "value = 1\n").unwrap();
        let edit = ValidatedVerifierRepairEdit::new(
            "main.py".to_string(),
            std::fs::canonicalize(&target).unwrap(),
            "value = 1\n",
            "value = 2\n".to_string(),
            "fp".to_string(),
        );

        let undo = apply_validated_repair_edit(&edit).unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "value = 2\n");
        assert_eq!(undo.relative_path, "main.py");
        assert_eq!(undo.preimage_contents, "value = 1\n");
        assert_eq!(undo.preimage_hash, edit.preimage_hash);
        assert_eq!(undo.postimage_hash, edit.postimage_hash);
    }

    #[test]
    fn reverts_when_postimage_matches() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("main.py");
        std::fs::write(&target, "value = 1\n").unwrap();
        let edit = ValidatedVerifierRepairEdit::new(
            "main.py".to_string(),
            std::fs::canonicalize(&target).unwrap(),
            "value = 1\n",
            "value = 2\n".to_string(),
            "fp".to_string(),
        );
        let undo = apply_validated_repair_edit(&edit).unwrap();

        revert_applied_repair_patch(&undo).unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "value = 1\n");
    }

    #[test]
    fn rejects_revert_when_postimage_changed() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("main.py");
        std::fs::write(&target, "value = 1\n").unwrap();
        let edit = ValidatedVerifierRepairEdit::new(
            "main.py".to_string(),
            std::fs::canonicalize(&target).unwrap(),
            "value = 1\n",
            "value = 2\n".to_string(),
            "fp".to_string(),
        );
        let undo = apply_validated_repair_edit(&edit).unwrap();
        std::fs::write(&target, "value = 3\n").unwrap();

        let err = revert_applied_repair_patch(&undo).unwrap_err();

        assert_eq!(err, "postimage changed after repair apply");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "value = 3\n");
    }

    #[test]
    fn rejects_when_preimage_changed() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("main.py");
        std::fs::write(&target, "value = 1\n").unwrap();
        let edit = ValidatedVerifierRepairEdit::new(
            "main.py".to_string(),
            std::fs::canonicalize(&target).unwrap(),
            "value = 1\n",
            "value = 2\n".to_string(),
            "fp".to_string(),
        );
        std::fs::write(&target, "value = 3\n").unwrap();

        let err = apply_validated_repair_edit(&edit).unwrap_err();

        assert_eq!(err, "preimage changed after validation");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "value = 3\n");
    }

    #[test]
    fn reports_read_failure_without_writing() {
        let edit = ValidatedVerifierRepairEdit::new(
            "missing.py".to_string(),
            PathBuf::from("/definitely/not/present/anvil-missing.py"),
            "",
            "value = 2\n".to_string(),
            "fp".to_string(),
        );

        let err = apply_validated_repair_edit(&edit).unwrap_err();

        assert!(err.contains("failed to read current target before apply"));
    }
}
