use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

use crate::planner::verify::VerificationReport;

pub mod checks;
mod claims_binding;
pub mod manifest;
pub mod results_schema;
pub mod runtime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSnapshot {
    pub protected_files: Vec<ProtectedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedFile {
    pub relative_path: String,
    pub size: u64,
    pub hash: u64,
}

pub fn verify(_root: &Path) -> VerificationReport {
    VerificationReport::pass()
}

pub fn before_phase(root: &Path) -> anyhow::Result<ProfileSnapshot> {
    let mut protected_files = Vec::new();
    for prefix in ["data/raw", "input"] {
        collect_files(root, &root.join(prefix), &mut protected_files)?;
    }
    protected_files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(ProfileSnapshot { protected_files })
}

pub fn after_phase(root: &Path, snapshot: &ProfileSnapshot) -> VerificationReport {
    let mut report = VerificationReport::pass();
    for file in &snapshot.protected_files {
        let path = root.join(&file.relative_path);
        let Ok(metadata) = std::fs::metadata(&path) else {
            report.push_profile_failure(format!(
                "protected data input removed: {}",
                file.relative_path
            ));
            continue;
        };
        if metadata.len() != file.size {
            report.push_profile_failure(format!(
                "protected data input size changed: {}",
                file.relative_path
            ));
            continue;
        }
        match file_hash(&path) {
            Ok(hash) if hash == file.hash => {}
            Ok(_) => report.push_profile_failure(format!(
                "protected data input content changed: {}",
                file.relative_path
            )),
            Err(err) => report.push_profile_failure(format!(
                "protected data input unreadable: {} ({err})",
                file.relative_path
            )),
        }
    }
    report
}

fn collect_files(
    root: &Path,
    dir: &Path,
    protected_files: &mut Vec<ProtectedFile>,
) -> anyhow::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files(root, &path, protected_files)?;
        } else if file_type.is_file() {
            let metadata = entry.metadata()?;
            protected_files.push(ProtectedFile {
                relative_path: crate::tools::path_guard::relative_display(root, &path),
                size: metadata.len(),
                hash: file_hash(&path)?,
            });
        }
    }
    Ok(())
}

fn file_hash(path: &Path) -> anyhow::Result<u64> {
    let bytes = std::fs::read(path)?;
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_profile_snapshots_raw_inputs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("data/raw")).unwrap();
        std::fs::write(dir.path().join("data/raw/source.csv"), "a,b\n1,2\n").unwrap();
        let snapshot = before_phase(dir.path()).unwrap();
        assert_eq!(snapshot.protected_files.len(), 1);
        assert_eq!(
            snapshot.protected_files[0].relative_path,
            "data/raw/source.csv"
        );
    }

    #[test]
    fn data_profile_rejects_raw_input_deletion() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("input")).unwrap();
        std::fs::write(dir.path().join("input/source.csv"), "a,b\n1,2\n").unwrap();
        let snapshot = before_phase(dir.path()).unwrap();
        std::fs::remove_file(dir.path().join("input/source.csv")).unwrap();
        let report = after_phase(dir.path(), &snapshot);
        assert!(!report.is_pass());
        assert!(report.primary_reason().contains("removed"));
    }

    #[test]
    fn data_profile_rejects_raw_input_hash_change() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("input")).unwrap();
        std::fs::write(dir.path().join("input/source.csv"), "1234").unwrap();
        let snapshot = before_phase(dir.path()).unwrap();
        std::fs::write(dir.path().join("input/source.csv"), "5678").unwrap();
        let report = after_phase(dir.path(), &snapshot);
        assert!(!report.is_pass());
        assert!(report.primary_reason().contains("content changed"));
    }

    #[test]
    fn data_profile_allows_derived_output_changes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("input")).unwrap();
        std::fs::write(dir.path().join("input/source.csv"), "1234").unwrap();
        let snapshot = before_phase(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join("output")).unwrap();
        std::fs::write(dir.path().join("output/result.csv"), "ok").unwrap();
        assert!(after_phase(dir.path(), &snapshot).is_pass());
    }

    #[test]
    fn domain_profile_hooks_are_supplied_by_the_embedded_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let profile = crate::planner::profile::domain_profile("data");

        assert_eq!(profile.guidance("goal"), Some(manifest::guidance()));
        assert_eq!(
            profile.runtime_contract("create", "goal"),
            manifest::runtime_contract()
        );
        assert_eq!(
            profile.generation_rules("create"),
            Some(manifest::generation_rules())
        );
        assert_eq!(
            profile
                .preset_ultra_plan("Analyze sales", "default", "create")
                .unwrap(),
            manifest::preset_ultra_plan("Analyze sales", "default", "create").unwrap()
        );
        assert_eq!(
            profile.expected_scaffold_paths(dir.path(), "goal"),
            manifest::required_artifacts()
        );
        assert_eq!(
            profile.infer_required_capabilities("goal"),
            manifest::required_capability_ids()
        );
        assert_eq!(
            profile.evidence_repair_target_paths(
                dir.path(),
                &["reconciliation_violation".to_string()]
            ),
            ["pipeline/main.py"]
        );
    }
}
