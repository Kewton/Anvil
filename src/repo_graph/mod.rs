//! `RepoGraph` v1: lightweight repo structure graph (Issue #468 / Epic E #448).
//!
//! Entry point: `build_repo_graph(work_root, state_root, &BuildOptions)`
//! returns a `BuildOutcome` (no event emit; the caller — typically
//! `Agent::ensure_repo_graph` — is the single facade for `agent.repo_graph.*`
//! event emission, DR1-005). The graph is read-only (`Arc<RepoGraph>`),
//! cached per-fingerprint under `state_root/repo_graph/`, and intentionally
//! independent of `agent` / `session` layers (DR1-001 / DR3-002).
//!
//! Public surface (DR1-004 / DR1-002 / DR1-003):
//!   - `build_repo_graph()` — value-returning builder
//!   - `BuildOutcome { Built / CacheHit / Skipped }`
//!   - `RepoGraph` — read-only methods: `nodes()`, `edges()`,
//!     `find_pairs(&Path) -> Vec<&Path>`, `imports_of(&Path) -> &[ImportRef]`
//!   - `RepoGraphError`
//!   - `BuildOptions` — only `max_files` / `max_depth` (test seam, no env field)
//!   - `ImportRef` / `ImportKind`

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};

pub(crate) mod fingerprint;
pub(crate) mod pair;
pub(crate) mod parse;
pub(crate) mod persist;

use fingerprint::{Fingerprint, current_repo_fingerprint};
use parse::{FragmentKind, LangKind, lang_from_path, parse_file};
use persist::{LoadOutcome, load, save};

// ---------------------------------------------------------------------------
// Public const SSOT (Issue #468 spec — Stage 1 S1-001 / S3-008)
// ---------------------------------------------------------------------------

/// Maximum number of files included in the graph. Workspaces with more files
/// are walked but truncated; the truncation triggers
/// `agent.repo_graph.skipped { reason='over_files_limit' }`.
pub const MAX_REPO_GRAPH_FILES: usize = 50_000;

/// Maximum walk depth from `work_root`.
pub const MAX_REPO_GRAPH_DEPTH: usize = 32;

// (per-file cap is `parse::MAX_REPO_GRAPH_FILE_BYTES`)
// (per-graph cap is `persist::MAX_REPO_GRAPH_TOTAL_BYTES`)
// (per-store cap is `persist::MAX_REPO_GRAPH_FILES_PERSISTED`)

const DENY_PREFIXES: &[&str] = &[
    "target/",
    "node_modules/",
    "__pycache__/",
    ".git/",
    ".next/",
    "build/",
    "dist/",
    ".anvil/",
    "dev-reports/",
];

// ---------------------------------------------------------------------------
// Public types (DR1-002 / DR1-003 / DR1-004 / S3-011)
// ---------------------------------------------------------------------------

/// Outcome of `build_repo_graph`. No event is emitted; the caller (a single
/// facade, typically `Agent::ensure_repo_graph`) is responsible for mapping
/// each variant to an `agent.repo_graph.*` log event (DR1-005 / DR2-004).
pub enum BuildOutcome {
    Built {
        graph: Arc<RepoGraph>,
        node_count: usize,
        edge_count: usize,
    },
    CacheHit {
        graph: Arc<RepoGraph>,
    },
    /// The build was bypassed for a non-error reason (cap exceeded, corrupt
    /// cache, dry-run). The string is the `reason` value the caller should
    /// log under `agent.repo_graph.skipped { reason }`.
    Skipped {
        reason: String,
    },
}

#[derive(Debug)]
pub enum RepoGraphError {
    /// `std::env::current_dir().canonicalize()` failed.
    CwdCanonicalFailed,
    /// Filesystem write failed (disk full, permission, oversize, etc.).
    PersistFailed(String),
    /// `ANVIL_NO_REPO_GRAPH` is set.
    Disabled,
}

#[derive(Default, Clone, Debug)]
pub struct BuildOptions {
    /// Override `MAX_REPO_GRAPH_FILES`. Default 0 means "use the const".
    pub max_files: usize,
    /// Override `MAX_REPO_GRAPH_DEPTH`. Default 0 means "use the const".
    pub max_depth: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRef {
    pub kind: ImportKind,
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ImportKind {
    RustUse,
    NodeImport,
    PythonImport,
}

// ---------------------------------------------------------------------------
// Internal types (pub(crate))
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Node {
    /// Path relative to `work_root`. Never absolute.
    pub(crate) path: PathBuf,
    pub(crate) kind: NodeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub(crate) enum NodeKind {
    File,
    Module,
    Symbol,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Edge {
    pub(crate) from: usize,
    pub(crate) to: usize,
    pub(crate) kind: EdgeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub(crate) enum EdgeKind {
    Imports,
    Defines,
    LikelyCovers,
}

// ---------------------------------------------------------------------------
// RepoGraph (read-only API)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct RepoGraph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    fingerprint: Fingerprint,
    /// Cached `imports_of` lookup: file index → ImportRef list.
    imports_index: HashMap<usize, Vec<ImportRef>>,
    /// Cached `find_pairs` lookup: test file path → list of impl paths.
    pairs_index: HashMap<PathBuf, Vec<PathBuf>>,
}

impl RepoGraph {
    /// Construction helper used by `persist::load` and the builder. Accepts
    /// raw nodes/edges and rebuilds the lookup indices.
    pub(crate) fn from_parts(fingerprint: Fingerprint, nodes: Vec<Node>, edges: Vec<Edge>) -> Self {
        let imports_index = build_imports_index(&nodes, &edges);
        let pairs_index = build_pairs_index(&nodes, &edges);
        Self {
            nodes,
            edges,
            fingerprint,
            imports_index,
            pairs_index,
        }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Returns relative paths of `LikelyCovers` impl candidates for the given
    /// `test_path` (workspace-relative). Returns an empty slice if the path
    /// is not a known test file.
    pub fn find_pairs(&self, test_path: &Path) -> Vec<&Path> {
        self.pairs_index
            .get(test_path)
            .map(|v| v.iter().map(|p| p.as_path()).collect())
            .unwrap_or_default()
    }

    /// Returns recorded imports originating from the given file (workspace-relative).
    pub fn imports_of(&self, path: &Path) -> &[ImportRef] {
        let idx = match self.nodes.iter().position(|n| n.path == path) {
            Some(i) => i,
            None => return &[],
        };
        self.imports_index
            .get(&idx)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub(crate) fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    pub(crate) fn nodes_internal(&self) -> &[Node] {
        &self.nodes
    }

    pub(crate) fn edges_internal(&self) -> &[Edge] {
        &self.edges
    }
}

fn build_imports_index(nodes: &[Node], edges: &[Edge]) -> HashMap<usize, Vec<ImportRef>> {
    let mut out: HashMap<usize, Vec<ImportRef>> = HashMap::new();
    for edge in edges.iter().filter(|e| e.kind == EdgeKind::Imports) {
        let from_node = match nodes.get(edge.from) {
            Some(n) => n,
            None => continue,
        };
        let to_node = match nodes.get(edge.to) {
            Some(n) => n,
            None => continue,
        };
        // Reconstruct ImportRef from the symbol Node's path string. Symbol
        // nodes embed `<lang>:<target>` so we can recover the original kind.
        let target_str = to_node.path.to_string_lossy();
        let (kind, target) = match target_str.split_once(':') {
            Some(("rust", t)) => (ImportKind::RustUse, t.to_string()),
            Some(("node", t)) => (ImportKind::NodeImport, t.to_string()),
            Some(("python", t)) => (ImportKind::PythonImport, t.to_string()),
            _ => continue,
        };
        out.entry(edge.from)
            .or_default()
            .push(ImportRef { kind, target });
        // suppress unused name warning on `from_node` while preserving lookup safety
        let _ = from_node;
    }
    out
}

fn build_pairs_index(nodes: &[Node], edges: &[Edge]) -> HashMap<PathBuf, Vec<PathBuf>> {
    let mut out: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for edge in edges.iter().filter(|e| e.kind == EdgeKind::LikelyCovers) {
        let from_node = match nodes.get(edge.from) {
            Some(n) => n,
            None => continue,
        };
        let to_node = match nodes.get(edge.to) {
            Some(n) => n,
            None => continue,
        };
        out.entry(from_node.path.clone())
            .or_default()
            .push(to_node.path.clone());
    }
    out
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Build (or load from cache) a `RepoGraph` for `work_root`, persisting the
/// result under `state_root/repo_graph/`. Returns `BuildOutcome` — the caller
/// is responsible for emitting the corresponding log event (DR1-005). Never
/// panics; any I/O failure is captured in `RepoGraphError::PersistFailed` or
/// degenerates to `BuildOutcome::Skipped`.
pub fn build_repo_graph(
    work_root: &Path,
    state_root: &Path,
    options: &BuildOptions,
) -> Result<BuildOutcome, RepoGraphError> {
    if std::env::var_os("ANVIL_NO_REPO_GRAPH").is_some() {
        return Err(RepoGraphError::Disabled);
    }

    let fp = current_repo_fingerprint(work_root, state_root)?;

    // Try cache first.
    match load(state_root, &fp) {
        LoadOutcome::Hit(boxed) => {
            return Ok(BuildOutcome::CacheHit {
                graph: Arc::new(*boxed),
            });
        }
        LoadOutcome::Corrupt => {
            // Best-effort: caller may emit `agent.repo_graph.skipped` with
            // reason='corrupt_cache' AND continue; we fall through to rebuild.
        }
        LoadOutcome::Miss => {}
    }

    let max_files = if options.max_files == 0 {
        MAX_REPO_GRAPH_FILES
    } else {
        options.max_files
    };
    let max_depth = if options.max_depth == 0 {
        MAX_REPO_GRAPH_DEPTH
    } else {
        options.max_depth
    };

    // Walk and collect candidate files.
    let walker = build_walker(work_root, state_root, max_depth);
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in walker.build().flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let rel = match path.strip_prefix(work_root) {
            Ok(r) => r.to_path_buf(),
            Err(_) => continue,
        };
        if is_denied_relative(&rel) {
            continue;
        }
        if files.len() >= max_files {
            return Ok(BuildOutcome::Skipped {
                reason: "over_files_limit".into(),
            });
        }
        files.push(rel);
    }

    // Build nodes and edges. Strategy:
    //   1. One File Node per discovered file.
    //   2. parse_file -> EdgeFragments. For Define, add a Symbol Node and
    //      a Defines edge. For Import, add a Module Node (key:
    //      "<lang>:<target>") and an Imports edge.
    //   3. Collect impl_file paths and run pair_test_with_impl on each
    //      test_file to add LikelyCovers edges.
    let mut nodes: Vec<Node> = Vec::with_capacity(files.len());
    let mut file_index: HashMap<PathBuf, usize> = HashMap::new();
    for f in &files {
        let idx = nodes.len();
        nodes.push(Node {
            path: f.clone(),
            kind: NodeKind::File,
        });
        file_index.insert(f.clone(), idx);
    }
    let mut edges: Vec<Edge> = Vec::new();
    let mut module_index: HashMap<String, usize> = HashMap::new();
    let mut impl_files: Vec<PathBuf> = Vec::new();

    for (file_idx, rel_path) in files.iter().enumerate() {
        let abs = work_root.join(rel_path);
        let lang = match lang_from_path(rel_path) {
            Some(l) => l,
            None => continue,
        };
        let lang_tag = match lang {
            LangKind::Rust => "rust",
            LangKind::Node => "node",
            LangKind::Python => "python",
        };
        let frags = parse_file(&abs, lang);
        for frag in frags {
            match frag.kind {
                FragmentKind::Import => {
                    let key = format!("{lang_tag}:{}", frag.target);
                    let to_idx = *module_index.entry(key.clone()).or_insert_with(|| {
                        let idx = nodes.len();
                        nodes.push(Node {
                            path: PathBuf::from(&key),
                            kind: NodeKind::Module,
                        });
                        idx
                    });
                    edges.push(Edge {
                        from: file_idx,
                        to: to_idx,
                        kind: EdgeKind::Imports,
                    });
                }
                FragmentKind::Define => {
                    let key = format!("{}::{}", rel_path.display(), frag.target);
                    let to_idx = nodes.len();
                    nodes.push(Node {
                        path: PathBuf::from(&key),
                        kind: NodeKind::Symbol,
                    });
                    edges.push(Edge {
                        from: file_idx,
                        to: to_idx,
                        kind: EdgeKind::Defines,
                    });
                }
            }
        }
        if crate::util::file_classify::is_implementation_file(rel_path)
            && !is_pairable_test(rel_path)
        {
            impl_files.push(rel_path.clone());
        }
    }

    // LikelyCovers pairing. We accept `is_test_file` (the SSOT, covering
    // `__tests__/`, `.test.`, `.spec.`) plus the Rust-/Python-conventional
    // `<stem>_test.<ext>` suffix that `file_classify` intentionally does
    // not recognise (DR1-007 / SSOT preserved).
    for rel_path in &files {
        if !is_pairable_test(rel_path) {
            continue;
        }
        let pairs = pair::pair_test_with_impl(rel_path, &impl_files);
        for impl_rel in pairs {
            let from_idx = match file_index.get(rel_path) {
                Some(i) => *i,
                None => continue,
            };
            let to_idx = match file_index.get(&impl_rel) {
                Some(i) => *i,
                None => continue,
            };
            edges.push(Edge {
                from: from_idx,
                to: to_idx,
                kind: EdgeKind::LikelyCovers,
            });
        }
    }

    let node_count = nodes.len();
    let edge_count = edges.len();
    let graph = RepoGraph::from_parts(fp, nodes, edges);
    let arc = Arc::new(graph);

    // Persist (best-effort: oversize is the only "skip" reason we surface here).
    if let Err(err) = save(state_root, &arc) {
        if let RepoGraphError::PersistFailed(reason) = &err
            && reason == "over_total_bytes"
        {
            return Ok(BuildOutcome::Skipped {
                reason: "over_total_bytes".into(),
            });
        }
        return Err(err);
    }

    Ok(BuildOutcome::Built {
        graph: arc,
        node_count,
        edge_count,
    })
}

/// Closure-DI helper for `ANVIL_REPO_GRAPH_DRY_RUN` (DR1-006). Currently
/// unused by `build_repo_graph` — the env gate is consulted directly when
/// callers want to skip persistence — but exposed for future test injection
/// and to mirror the case_record / tester DI seam.
#[allow(dead_code)]
pub(crate) fn repo_graph_dry_run<F>(read_env: F) -> bool
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    matches!(read_env("ANVIL_REPO_GRAPH_DRY_RUN"), Ok(v) if !v.is_empty())
}

fn build_walker(work_root: &Path, state_root: &Path, max_depth: usize) -> WalkBuilder {
    let mut b = WalkBuilder::new(work_root);
    b.git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .hidden(true)
        .follow_links(false)
        .max_depth(Some(max_depth));

    // Dogfood deny: if state_root sits under work_root, skip it.
    if let (Ok(work_canonical), Ok(state_canonical)) =
        (work_root.canonicalize(), state_root.canonicalize())
        && let Ok(rel) = state_canonical.strip_prefix(&work_canonical)
        && !rel.as_os_str().is_empty()
    {
        let denied = rel.to_path_buf();
        b.filter_entry(move |e| {
            let p = e.path();
            !p.components().zip(denied.components()).all(|(a, b)| a == b)
                || p.components().count() < denied.components().count()
        });
    }

    b
}

fn is_pairable_test(rel: &Path) -> bool {
    if crate::util::file_classify::is_test_file(rel) {
        return true;
    }
    // Graph-internal heuristic: `<stem>_test.<ext>` (Rust / Python convention
    // not covered by `file_classify`'s SSOT). This is read-only relative to
    // the SSOT — `file_classify` is unchanged.
    let stem = match rel.file_stem().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return false,
    };
    stem.ends_with("_test")
}

fn is_denied_relative(rel: &Path) -> bool {
    let s = rel.to_string_lossy();
    DENY_PREFIXES
        .iter()
        .any(|p| s.starts_with(p) || s.contains(&format!("/{p}")))
}

// Manual Debug for BuildOutcome (Arc<RepoGraph> doesn't auto-derive a useful Debug for our purposes).
impl std::fmt::Debug for BuildOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildOutcome::Built {
                node_count,
                edge_count,
                ..
            } => f
                .debug_struct("Built")
                .field("node_count", node_count)
                .field("edge_count", edge_count)
                .finish(),
            BuildOutcome::CacheHit { .. } => f.debug_struct("CacheHit").finish(),
            BuildOutcome::Skipped { reason } => {
                f.debug_struct("Skipped").field("reason", reason).finish()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_dir() -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "anvil-rg-build-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn dry_run_helper_responds_to_closure() {
        assert!(repo_graph_dry_run(|_| Ok("1".to_string())));
        assert!(!repo_graph_dry_run(|_| Err(std::env::VarError::NotPresent)));
        assert!(!repo_graph_dry_run(|_| Ok(String::new())));
    }

    #[test]
    fn deny_relative_catches_known_dirs() {
        assert!(is_denied_relative(Path::new("target/foo.rs")));
        assert!(is_denied_relative(Path::new("node_modules/x")));
        assert!(is_denied_relative(Path::new(".git/HEAD")));
        assert!(!is_denied_relative(Path::new("src/lib.rs")));
    }

    /// All `build_repo_graph` smoke tests are consolidated into a single
    /// sequential test because they mutate process-wide environment
    /// variables (`ANVIL_NO_REPO_GRAPH`). Splitting them across `#[test]`
    /// functions would race under cargo's default parallel test runner.
    #[test]
    fn builder_smoke_suite() {
        // Ensure a clean baseline.
        unsafe { std::env::remove_var("ANVIL_NO_REPO_GRAPH") };

        // 1) Built outcome on a minimal Rust workspace.
        {
            let tmp = fixture_dir();
            let work = tmp.join("ws");
            std::fs::create_dir_all(work.join("src")).unwrap();
            std::fs::write(work.join("src/lib.rs"), b"pub fn hi() {}\n").unwrap();
            std::fs::write(work.join("src/lib_test.rs"), b"use crate::hi;\n").unwrap();
            let state = tmp.join("state");
            std::fs::create_dir_all(&state).unwrap();
            let outcome = build_repo_graph(&work, &state, &BuildOptions::default()).unwrap();
            match outcome {
                BuildOutcome::Built {
                    graph, node_count, ..
                } => {
                    assert!(node_count >= 2, "expected 2+ nodes, got {node_count}");
                    assert!(graph.node_count() >= 2);
                }
                other => panic!("expected Built, got {other:?}"),
            }
            let _ = std::fs::remove_dir_all(&tmp);
        }

        // 2) Second build hits the persisted cache.
        {
            let tmp = fixture_dir();
            let work = tmp.join("ws");
            std::fs::create_dir_all(&work).unwrap();
            std::fs::write(work.join("a.rs"), b"pub fn a() {}\n").unwrap();
            let state = tmp.join("state");
            std::fs::create_dir_all(&state).unwrap();
            let _ = build_repo_graph(&work, &state, &BuildOptions::default()).unwrap();
            let outcome2 = build_repo_graph(&work, &state, &BuildOptions::default()).unwrap();
            assert!(matches!(outcome2, BuildOutcome::CacheHit { .. }));
            let _ = std::fs::remove_dir_all(&tmp);
        }

        // 3) Over-files-limit returns Skipped with the documented reason.
        {
            let tmp = fixture_dir();
            let work = tmp.join("ws");
            std::fs::create_dir_all(&work).unwrap();
            for i in 0..3 {
                std::fs::write(work.join(format!("a{i}.rs")), b"pub fn a() {}\n").unwrap();
            }
            let state = tmp.join("state");
            std::fs::create_dir_all(&state).unwrap();
            let opts = BuildOptions {
                max_files: 2,
                max_depth: 0,
            };
            let outcome = build_repo_graph(&work, &state, &opts).unwrap();
            assert!(
                matches!(outcome, BuildOutcome::Skipped { reason } if reason == "over_files_limit"),
            );
            let _ = std::fs::remove_dir_all(&tmp);
        }

        // 4) `ANVIL_NO_REPO_GRAPH` short-circuits to `Err(Disabled)`.
        {
            // SAFETY: tests in this module are serialised by being inside one
            // `#[test]` function; no other thread can race the env mutation.
            unsafe { std::env::set_var("ANVIL_NO_REPO_GRAPH", "1") };
            let tmp = fixture_dir();
            let work = tmp.join("ws");
            std::fs::create_dir_all(&work).unwrap();
            let state = tmp.join("state");
            std::fs::create_dir_all(&state).unwrap();
            let outcome = build_repo_graph(&work, &state, &BuildOptions::default());
            unsafe { std::env::remove_var("ANVIL_NO_REPO_GRAPH") };
            assert!(matches!(outcome, Err(RepoGraphError::Disabled)));
            let _ = std::fs::remove_dir_all(&tmp);
        }
    }
}
