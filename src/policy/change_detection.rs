use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeDetectionMethod {
    ToolReported,
    TargetedSnapshotDiff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSnapshot {
    pub entries: BTreeMap<PathBuf, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedPath {
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ChangeDetection {
    method: ChangeDetectionMethod,
}

impl ChangeDetection {
    pub fn new(method: ChangeDetectionMethod) -> Self {
        Self { method }
    }

    pub fn snapshot(&self, paths: &[PathBuf]) -> anyhow::Result<FileSnapshot> {
        let mut entries = BTreeMap::new();
        if matches!(self.method, ChangeDetectionMethod::TargetedSnapshotDiff) {
            for path in paths {
                entries.insert(path.clone(), digest_path(path)?);
            }
        }
        Ok(FileSnapshot { entries })
    }

    pub fn diff(
        &self,
        before: &FileSnapshot,
        paths: &[PathBuf],
    ) -> anyhow::Result<Vec<ChangedPath>> {
        let mut changed = Vec::new();
        match self.method {
            ChangeDetectionMethod::ToolReported => {}
            ChangeDetectionMethod::TargetedSnapshotDiff => {
                for path in paths {
                    let after = digest_path(path)?;
                    let before_digest = before.entries.get(path).copied().unwrap_or_default();
                    if before_digest != after {
                        changed.push(ChangedPath { path: path.clone() });
                    }
                }
            }
        }
        Ok(changed)
    }

    pub fn from_reported(&self, paths: Vec<PathBuf>) -> Vec<ChangedPath> {
        paths.into_iter().map(|path| ChangedPath { path }).collect()
    }
}

fn digest_path(path: &Path) -> anyhow::Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let bytes = std::fs::read(path)?;
    let mut hash = 1469598103934665603_u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(1099511628211);
    }
    Ok(hash)
}
