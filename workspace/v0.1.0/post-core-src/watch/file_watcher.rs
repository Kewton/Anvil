use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ignore::WalkBuilder;

#[derive(Debug, Clone)]
pub struct FileWatcher {
    root: PathBuf,
    snapshot: BTreeMap<PathBuf, SystemTime>,
}

impl FileWatcher {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        let snapshot = scan_snapshot(&root)?;
        Ok(Self { root, snapshot })
    }

    pub fn poll_changes(&mut self) -> Result<Vec<String>, String> {
        let next = scan_snapshot(&self.root)?;
        let mut changes = Vec::new();

        for (path, modified) in &next {
            match self.snapshot.get(path) {
                Some(previous) if previous == modified => {}
                Some(_) => changes.push(format!("modified {}", path.display())),
                None => changes.push(format!("added {}", path.display())),
            }
        }

        for path in self.snapshot.keys() {
            if !next.contains_key(path) {
                changes.push(format!("removed {}", path.display()));
            }
        }

        self.snapshot = next;
        changes.sort();
        Ok(changes)
    }
}

fn scan_snapshot(root: &Path) -> Result<BTreeMap<PathBuf, SystemTime>, String> {
    let mut snapshot = BTreeMap::new();
    for entry in WalkBuilder::new(root).hidden(false).build() {
        let entry = entry.map_err(|err| format!("watch scan error: {err}"))?;
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .to_path_buf();
        if should_skip(&relative) {
            continue;
        }
        let modified = fs::metadata(entry.path())
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        snapshot.insert(relative, modified);
    }
    Ok(snapshot)
}

fn should_skip(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        matches!(name.as_ref(), ".git" | ".anvil" | "target" | "node_modules")
    })
}
