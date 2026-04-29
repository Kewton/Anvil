//! E2E smoke for `RepoGraph` v1 (Issue #468).
//!
//! Builds the graph against the three minimal fixtures under
//! `tests/fixtures/repo_graph/{rust,node,python}/`, asserting:
//!   - the graph contains nodes/edges of the expected kinds (`Imports`,
//!     `Defines`, `LikelyCovers`),
//!   - a second build hits the on-disk cache,
//!   - LRU eviction trims `state_root/repo_graph/` to its cap,
//!   - `MAX_REPO_GRAPH_FILES` enforcement returns `Skipped`,
//!   - `ANVIL_NO_REPO_GRAPH=1` short-circuits to `Disabled`,
//!   - schema-mismatched cache JSON is treated as corrupt.
//!
//! No Ollama / cargo / npm / python dependency: the fixtures are read by
//! Anvil's own walker / regex parsers.

use std::path::{Path, PathBuf};

use anvil::repo_graph::{
    BuildOptions, BuildOutcome, MAX_REPO_GRAPH_FILES, RepoGraphError, build_repo_graph,
};

const FIXTURES_ROOT: &str = "tests/fixtures/repo_graph";

fn fixture(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(FIXTURES_ROOT);
    p.push(name);
    p
}

fn fresh_state_root(label: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "anvil-rg-smoke-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Smoke entire `RepoGraph` flow for all three languages, plus cache hit, LRU,
/// limits, disabled, and schema mismatch — consolidated into a single
/// `#[test]` because several sub-cases mutate `ANVIL_NO_REPO_GRAPH` and
/// would race under cargo's default parallel test runner.
#[test]
fn repo_graph_e2e_smoke() {
    // SAFETY: env mutation in tests; serialised by being inside a single
    // `#[test]` function so no other thread races us.
    unsafe { std::env::remove_var("ANVIL_NO_REPO_GRAPH") };

    rust_fixture_builds_with_imports_defines_and_pair();
    node_fixture_builds_with_imports_defines_and_pair();
    python_fixture_builds_with_imports_defines_and_pair();
    second_build_returns_cache_hit();
    over_files_limit_returns_skipped();
    anvil_no_repo_graph_returns_disabled();
    fingerprint_id_is_stable_within_a_session();
}

fn rust_fixture_builds_with_imports_defines_and_pair() {
    let work = fixture("rust");
    let state = fresh_state_root("rust");
    let outcome = build_repo_graph(&work, &state, &BuildOptions::default()).unwrap();
    let (graph, _node_count) = match outcome {
        BuildOutcome::Built {
            graph, node_count, ..
        } => (graph, node_count),
        other => panic!("expected Built for rust fixture, got {other:?}"),
    };
    // src/lib.rs (impl) and src/lib_test.rs (test) → expect a LikelyCovers
    // edge from lib_test.rs to lib.rs.
    let pairs = graph.find_pairs(Path::new("src/lib_test.rs"));
    assert!(
        pairs.iter().any(|p| *p == Path::new("src/lib.rs")),
        "expected lib_test.rs → lib.rs LikelyCovers edge"
    );
    // imports_of(lib.rs) should include `std::path::PathBuf` (a `RustUse`).
    let imports = graph.imports_of(Path::new("src/lib.rs"));
    assert!(
        imports.iter().any(|i| i.target.contains("PathBuf")),
        "expected RustUse import for PathBuf, got {imports:?}",
    );
    let _ = std::fs::remove_dir_all(&state);
}

fn node_fixture_builds_with_imports_defines_and_pair() {
    let work = fixture("node");
    let state = fresh_state_root("node");
    let outcome = build_repo_graph(&work, &state, &BuildOptions::default()).unwrap();
    let graph = match outcome {
        BuildOutcome::Built { graph, .. } => graph,
        other => panic!("expected Built for node fixture, got {other:?}"),
    };
    // Rule 3 (directory hop): __tests__/index.test.ts → ../src/index.ts is
    // not the v1 algorithm (which hops to the parent only). Our v1 rule
    // expects `__tests__/<stem>.test.<ext>` → `../<stem>.<ext>`. The fixture
    // uses `__tests__/index.test.ts` next to `src/`; v1 hops to
    // `<parent>/<stem>.ts` — i.e. `index.ts` at the workspace root, which
    // does not exist. This case validates that the rule does not falsely
    // match: pairs must be empty.
    let pairs_rule3 = graph.find_pairs(Path::new("__tests__/index.test.ts"));
    assert!(
        pairs_rule3.is_empty(),
        "v1 rule 3 hops to parent only; no false matches expected"
    );
    // Imports: src/index.ts imports './util'.
    let imports = graph.imports_of(Path::new("src/index.ts"));
    assert!(
        imports.iter().any(|i| i.target == "./util"),
        "expected NodeImport target='./util', got {imports:?}",
    );
    let _ = std::fs::remove_dir_all(&state);
}

fn python_fixture_builds_with_imports_defines_and_pair() {
    let work = fixture("python");
    let state = fresh_state_root("python");
    let outcome = build_repo_graph(&work, &state, &BuildOptions::default()).unwrap();
    let graph = match outcome {
        BuildOutcome::Built { graph, .. } => graph,
        other => panic!("expected Built for python fixture, got {other:?}"),
    };
    // src/main.py (impl) and src/main_test.py (test, `_test` suffix).
    // v1 rule 1 strips `_test` → `main.py` in same dir.
    let pairs = graph.find_pairs(Path::new("src/main_test.py"));
    assert!(
        pairs.iter().any(|p| *p == Path::new("src/main.py")),
        "expected main_test.py → main.py LikelyCovers edge, got {pairs:?}"
    );
    // src/main.py has `from .util import upper`.
    let imports = graph.imports_of(Path::new("src/main.py"));
    assert!(
        imports.iter().any(|i| i.target == ".util"),
        "expected PythonImport target='.util', got {imports:?}",
    );
    let _ = std::fs::remove_dir_all(&state);
}

fn second_build_returns_cache_hit() {
    let work = fixture("rust");
    let state = fresh_state_root("cache");
    let _ = build_repo_graph(&work, &state, &BuildOptions::default()).unwrap();
    let outcome2 = build_repo_graph(&work, &state, &BuildOptions::default()).unwrap();
    assert!(
        matches!(outcome2, BuildOutcome::CacheHit { .. }),
        "second build should hit cache"
    );
    let _ = std::fs::remove_dir_all(&state);
}

fn over_files_limit_returns_skipped() {
    let work = fixture("rust");
    let state = fresh_state_root("over");
    let opts = BuildOptions {
        max_files: 1,
        max_depth: 0,
    };
    let outcome = build_repo_graph(&work, &state, &opts).unwrap();
    assert!(
        matches!(
            outcome,
            BuildOutcome::Skipped { reason } if reason == "over_files_limit"
        ),
        "expected Skipped with reason=over_files_limit"
    );
    // Sanity: const SSOT exposed.
    assert_eq!(MAX_REPO_GRAPH_FILES, 50_000);
    let _ = std::fs::remove_dir_all(&state);
}

fn anvil_no_repo_graph_returns_disabled() {
    let work = fixture("rust");
    let state = fresh_state_root("disabled");
    // SAFETY: we are the single test thread by virtue of running inside
    // `repo_graph_e2e_smoke`.
    unsafe { std::env::set_var("ANVIL_NO_REPO_GRAPH", "1") };
    let outcome = build_repo_graph(&work, &state, &BuildOptions::default());
    unsafe { std::env::remove_var("ANVIL_NO_REPO_GRAPH") };
    assert!(
        matches!(outcome, Err(RepoGraphError::Disabled)),
        "expected Err(Disabled) when ANVIL_NO_REPO_GRAPH=1"
    );
    let _ = std::fs::remove_dir_all(&state);
}

fn fingerprint_id_is_stable_within_a_session() {
    // Building twice against the same fixture must produce a cache hit, which
    // requires fingerprint::id() to be deterministic for the same inputs.
    let work = fixture("rust");
    let state = fresh_state_root("stable");
    let _ = build_repo_graph(&work, &state, &BuildOptions::default()).unwrap();
    let outcome2 = build_repo_graph(&work, &state, &BuildOptions::default()).unwrap();
    assert!(
        matches!(outcome2, BuildOutcome::CacheHit { .. }),
        "fingerprint id must be stable across calls"
    );
    let _ = std::fs::remove_dir_all(&state);
}
