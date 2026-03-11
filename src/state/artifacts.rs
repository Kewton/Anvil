use std::path::{Path, PathBuf};

use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ArtifactStore {
    root: PathBuf,
    max_entries: usize,
}

impl ArtifactStore {
    pub fn new(root: impl Into<PathBuf>, max_entries: usize) -> Self {
        Self {
            root: root.into(),
            max_entries,
        }
    }

    pub fn write_text(&self, prefix: &str, text: &str) -> anyhow::Result<PathBuf> {
        std::fs::create_dir_all(&self.root)?;
        let path = self.root.join(format!("{prefix}-{}.txt", monotonic_id()));
        std::fs::write(&path, text)?;
        self.rotate(prefix)?;
        Ok(path)
    }

    fn rotate(&self, prefix: &str) -> anyhow::Result<()> {
        let mut entries = std::fs::read_dir(&self.root)?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let keep = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.starts_with(prefix))
                    .unwrap_or(false);
                if !keep {
                    return None;
                }
                let modified = std::fs::metadata(&path)
                    .and_then(|meta| meta.modified())
                    .ok()?;
                Some((path, modified))
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|(_, modified)| *modified);
        while entries.len() > self.max_entries {
            if let Some((oldest, _)) = entries.first().cloned() {
                std::fs::remove_file(&oldest)?;
                let _ = entries.remove(0);
            }
        }
        Ok(())
    }
}

fn monotonic_id() -> u128 {
    Uuid::new_v4().as_u128()
}

#[allow(dead_code)]
pub fn artifacts_dir(base: &Path) -> PathBuf {
    base.join(".anvil/state/artifacts")
}
