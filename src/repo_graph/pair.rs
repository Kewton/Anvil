//! `likely_covers` test↔implementation pairing for `RepoGraph` v1
//! (Issue #468 / S3-005 / DR1-004).
//!
//! Three v1 rules are applied in order; all matching candidates are returned
//! (multi-candidate support, DR1-004). The classification (`is_test_file` /
//! `is_implementation_file`) is delegated to the `util::file_classify` SSOT
//! (#456) — this module never re-implements that judgement.
//!
//! Rules:
//!   1. basename suffix strip: `foo_test.rs` → `foo.rs`
//!   2. dotted suffix strip: `foo.test.ts` → `foo.ts`, `foo.spec.py` → `foo.py`
//!   3. directory hop: `__tests__/foo.test.ts` → `../foo.ts`

use std::path::{Path, PathBuf};

use crate::util::file_classify::is_implementation_file;

/// Find all `implementation_file` candidates that the given `test_path` likely
/// covers, considering only entries from `candidates` (typically the full
/// implementation-file set discovered during the walk). Returns paths that
/// actually appear in `candidates` to avoid hallucinating files.
///
/// Note: this function intentionally does NOT gate on
/// `util::file_classify::is_test_file`. Rule 1 (`foo_test.rs` → `foo.rs`) is
/// the Rust-specific test convention which `file_classify` does not classify
/// as a test file (it only recognises `__tests__`, `.test.`, `.spec.`). The
/// rules here are specific enough that they will not fire for ordinary
/// implementation files, so callers can hand any walk-discovered file in;
/// rules that do not apply simply produce no edges.
pub(crate) fn pair_test_with_impl(test_path: &Path, candidates: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();

    let parent = test_path.parent().unwrap_or(Path::new(""));
    let stem = match test_path.file_stem().and_then(|s| s.to_str()) {
        Some(s) => s,
        None => return out,
    };
    let ext = test_path.extension().and_then(|s| s.to_str()).unwrap_or("");

    // Rule 1: basename suffix strip — `foo_test.rs` → `foo.rs`
    if let Some(base) = stem.strip_suffix("_test") {
        let cand = parent.join(format!("{base}.{ext}"));
        if candidates.iter().any(|p| p == &cand) && is_implementation_file(&cand) {
            out.push(cand);
        }
    }

    // Rule 2: dotted suffix strip — `foo.test.ts` → `foo.ts`, `foo.spec.py` → `foo.py`
    for marker in [".test", ".spec"] {
        if let Some(base) = stem.strip_suffix(marker) {
            let cand = parent.join(format!("{base}.{ext}"));
            if candidates.iter().any(|p| p == &cand)
                && is_implementation_file(&cand)
                && !out.contains(&cand)
            {
                out.push(cand);
            }
        }
    }

    // Rule 3: directory hop — `__tests__/foo.test.ts` → `../foo.ts`
    if parent
        .file_name()
        .and_then(|s| s.to_str())
        .map(|n| n == "__tests__")
        .unwrap_or(false)
    {
        let parent_parent = parent.parent().unwrap_or(Path::new(""));
        // try with dotted suffix stripped first (`foo.test` → `foo`)
        let candidate_stem: &str = stem.strip_suffix(".test").unwrap_or(stem);
        let cand = parent_parent.join(format!("{candidate_stem}.{ext}"));
        if candidates.iter().any(|p| p == &cand)
            && is_implementation_file(&cand)
            && !out.contains(&cand)
        {
            out.push(cand);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn rule1_basename_suffix_strip() {
        let candidates = vec![p("src/foo.rs"), p("src/bar.rs")];
        let pairs = pair_test_with_impl(&p("src/foo_test.rs"), &candidates);
        assert_eq!(pairs, vec![p("src/foo.rs")]);
    }

    #[test]
    fn rule2_dotted_suffix_strip_node() {
        let candidates = vec![p("src/foo.ts")];
        let pairs = pair_test_with_impl(&p("src/foo.test.ts"), &candidates);
        assert_eq!(pairs, vec![p("src/foo.ts")]);
    }

    #[test]
    fn rule2_dotted_suffix_strip_python() {
        let candidates = vec![p("src/foo.py")];
        let pairs = pair_test_with_impl(&p("src/foo.spec.py"), &candidates);
        assert_eq!(pairs, vec![p("src/foo.py")]);
    }

    #[test]
    fn rule3_directory_hop() {
        let candidates = vec![p("src/foo.ts")];
        let pairs = pair_test_with_impl(&p("src/__tests__/foo.test.ts"), &candidates);
        assert_eq!(pairs, vec![p("src/foo.ts")]);
    }

    #[test]
    fn returns_empty_when_no_candidate_matches() {
        let candidates = vec![p("src/other.rs")];
        let pairs = pair_test_with_impl(&p("src/foo_test.rs"), &candidates);
        assert!(pairs.is_empty());
    }

    #[test]
    fn returns_empty_for_ordinary_impl_file_with_no_test_marker() {
        // foo.rs has no `_test`/`.test.`/`.spec.` marker and is not under
        // `__tests__/` so none of the three rules fire.
        let candidates = vec![p("src/foo.rs")];
        let pairs = pair_test_with_impl(&p("src/foo.rs"), &candidates);
        assert!(pairs.is_empty());
    }
}
