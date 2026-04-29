//! `state_root/repo_graph/<fingerprint_id>.json` persistence with LRU
//! eviction and lossy / corrupt-skip load (Issue #468 / S3-008 / DR1-008).
//!
//! Schema progression: every persisted file carries a top-level
//! `schema_version: u8`. On load, mismatch is treated as corrupt and the
//! file is best-effort removed (`agent.repo_graph.skipped { reason='corrupt_cache' }`
//! is emitted by the caller). At write-time, exceeding
//! `MAX_REPO_GRAPH_FILES_PERSISTED` triggers eviction of the oldest mtime
//! entries (CaseRecord-style, #462). Total per-file size is capped by
//! `MAX_REPO_GRAPH_TOTAL_BYTES` (16 MiB).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::fingerprint::{Fingerprint, SCHEMA_VERSION};
use super::{Edge, Node, RepoGraph, RepoGraphError};

/// Per-file size cap for a single persisted graph (16 MiB).
pub(crate) const MAX_REPO_GRAPH_TOTAL_BYTES: usize = 16 * 1024 * 1024;

/// Number of fingerprint files retained under `state_root/repo_graph/`.
/// Exceeding this triggers LRU eviction.
pub(crate) const MAX_REPO_GRAPH_FILES_PERSISTED: usize = 8;

/// Wire format. `schema_version` is duplicated outside the embedded
/// fingerprint so unknown / mismatched versions can be detected even if the
/// inner deserialization succeeds opportunistically.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PersistedRepoGraph {
    pub(crate) schema_version: u8,
    pub(crate) fingerprint: Fingerprint,
    pub(crate) nodes: Vec<Node>,
    pub(crate) edges: Vec<Edge>,
}

/// Save `graph` to `state_root/repo_graph/<id>.json`. Performs LRU eviction
/// before writing. Returns `Err(PersistFailed)` on size cap overrun or I/O
/// failure.
pub(crate) fn save(state_root: &Path, graph: &RepoGraph) -> Result<(), RepoGraphError> {
    let dir = state_root.join("repo_graph");
    fs::create_dir_all(&dir).map_err(|e| RepoGraphError::PersistFailed(e.to_string()))?;

    let entries = list_repo_graph_files(&dir);
    if entries.len() >= MAX_REPO_GRAPH_FILES_PERSISTED {
        // Make room for one new file.
        let to_evict = entries.len() + 1 - MAX_REPO_GRAPH_FILES_PERSISTED;
        evict_oldest_by_mtime(&entries, to_evict);
    }

    let payload = PersistedRepoGraph {
        schema_version: SCHEMA_VERSION,
        fingerprint: graph.fingerprint().clone(),
        nodes: graph.nodes_internal().to_vec(),
        edges: graph.edges_internal().to_vec(),
    };
    let bytes =
        serde_json::to_vec(&payload).map_err(|e| RepoGraphError::PersistFailed(e.to_string()))?;
    if bytes.len() > MAX_REPO_GRAPH_TOTAL_BYTES {
        return Err(RepoGraphError::PersistFailed("over_total_bytes".into()));
    }

    let path = dir.join(format!("{}.json", graph.fingerprint().id()));
    fs::write(&path, bytes).map_err(|e| RepoGraphError::PersistFailed(e.to_string()))
}

/// Try to load a previously persisted graph for `fp`. Returns `Hit(_)` on
/// successful load, `Miss` when no file exists, `Corrupt` on schema
/// mismatch or unparseable JSON (the caller treats `Corrupt` as a
/// `corrupt_cache` skip and emits the matching event after best-effort
/// `remove_file`).
///
/// `Hit` boxes `RepoGraph` to keep `LoadOutcome` small (clippy
/// `large_enum_variant`).
pub(crate) enum LoadOutcome {
    Hit(Box<RepoGraph>),
    Miss,
    Corrupt,
}

pub(crate) fn load(state_root: &Path, fp: &Fingerprint) -> LoadOutcome {
    let path = state_root
        .join("repo_graph")
        .join(format!("{}.json", fp.id()));
    let meta = match fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => return LoadOutcome::Miss,
    };
    if meta.len() as usize > MAX_REPO_GRAPH_TOTAL_BYTES {
        let _ = fs::remove_file(&path);
        return LoadOutcome::Corrupt;
    }
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(_) => return LoadOutcome::Miss,
    };
    let parsed: Result<PersistedRepoGraph, _> = serde_json::from_slice(&bytes);
    match parsed {
        Ok(p) if p.schema_version == SCHEMA_VERSION && &p.fingerprint == fp => LoadOutcome::Hit(
            Box::new(RepoGraph::from_parts(p.fingerprint, p.nodes, p.edges)),
        ),
        Ok(_) => {
            let _ = fs::remove_file(&path);
            LoadOutcome::Corrupt
        }
        Err(_) => {
            let _ = fs::remove_file(&path);
            LoadOutcome::Corrupt
        }
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    path: PathBuf,
    mtime: SystemTime,
}

fn list_repo_graph_files(dir: &Path) -> Vec<CacheEntry> {
    let mut out = Vec::new();
    let read = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return out,
    };
    for entry in read.flatten() {
        let path = entry.path();
        let meta = match entry.file_type() {
            Ok(t) if t.is_file() => match fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            },
            _ => continue,
        };
        // Reject non-`.json` to avoid touching unrelated junk.
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        out.push(CacheEntry { path, mtime });
    }
    out.sort_by_key(|e| e.mtime);
    out
}

fn evict_oldest_by_mtime(entries: &[CacheEntry], n: usize) {
    for entry in entries.iter().take(n) {
        let _ = fs::remove_file(&entry.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo_graph::{Edge, EdgeKind, Node, NodeKind, RepoGraph};
    use std::thread::sleep;
    use std::time::Duration;

    fn fake_fingerprint(seed: u8) -> Fingerprint {
        Fingerprint {
            git_rev: Some(format!("rev{seed:02x}")),
            work_root_hash: format!("ww{seed:02x}aaaaaaaa"),
            state_root_hash: format!("ss{seed:02x}aaaaaaaa"),
            schema_version: SCHEMA_VERSION,
        }
    }

    fn fake_graph(seed: u8) -> RepoGraph {
        let nodes = vec![Node {
            path: PathBuf::from("a.rs"),
            kind: NodeKind::File,
        }];
        let edges = vec![Edge {
            from: 0,
            to: 0,
            kind: EdgeKind::Defines,
        }];
        RepoGraph::from_parts(fake_fingerprint(seed), nodes, edges)
    }

    #[test]
    fn round_trip_save_and_load() {
        let tmp = std::env::temp_dir().join(format!(
            "anvil-rg-persist-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let g = fake_graph(1);
        save(&tmp, &g).unwrap();
        let loaded = load(&tmp, g.fingerprint());
        match loaded {
            LoadOutcome::Hit(rg) => {
                assert_eq!(rg.nodes_internal().len(), 1);
                assert_eq!(rg.edges_internal().len(), 1);
            }
            _ => panic!("expected Hit"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn miss_when_no_file() {
        let tmp = std::env::temp_dir().join(format!("anvil-rg-miss-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let outcome = load(&tmp, &fake_fingerprint(7));
        matches!(outcome, LoadOutcome::Miss);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn corrupt_json_returns_corrupt_and_removes_file() {
        let tmp = std::env::temp_dir().join(format!(
            "anvil-rg-corrupt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(tmp.join("repo_graph")).unwrap();
        let fp = fake_fingerprint(2);
        let p = tmp.join("repo_graph").join(format!("{}.json", fp.id()));
        std::fs::write(&p, b"not json").unwrap();
        let outcome = load(&tmp, &fp);
        assert!(matches!(outcome, LoadOutcome::Corrupt));
        assert!(!p.exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn lru_evicts_oldest_at_9th_save() {
        let tmp = std::env::temp_dir().join(format!(
            "anvil-rg-lru-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        for i in 0..(MAX_REPO_GRAPH_FILES_PERSISTED as u8) {
            save(&tmp, &fake_graph(i)).unwrap();
            sleep(Duration::from_millis(2));
        }
        let dir = tmp.join("repo_graph");
        let count_before = list_repo_graph_files(&dir).len();
        assert_eq!(count_before, MAX_REPO_GRAPH_FILES_PERSISTED);
        // 9th save.
        save(&tmp, &fake_graph(99)).unwrap();
        let count_after = list_repo_graph_files(&dir).len();
        assert_eq!(count_after, MAX_REPO_GRAPH_FILES_PERSISTED);
        // The oldest seed (0) must be gone.
        let oldest_path = dir.join(format!("{}.json", fake_fingerprint(0).id()));
        assert!(
            !oldest_path.exists(),
            "oldest entry should have been evicted"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn schema_mismatch_returns_corrupt() {
        let tmp = std::env::temp_dir().join(format!(
            "anvil-rg-schema-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(tmp.join("repo_graph")).unwrap();
        let fp = fake_fingerprint(3);
        let path = tmp.join("repo_graph").join(format!("{}.json", fp.id()));
        // Hand-write a payload with a different schema_version.
        let bad = serde_json::json!({
            "schema_version": 99,
            "fingerprint": &fp,
            "nodes": [],
            "edges": [],
        });
        std::fs::write(&path, serde_json::to_vec(&bad).unwrap()).unwrap();
        let outcome = load(&tmp, &fp);
        assert!(matches!(outcome, LoadOutcome::Corrupt));
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
