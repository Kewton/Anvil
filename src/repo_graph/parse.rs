//! Language-specific regex parsers for `RepoGraph` v1 (Issue #468 / DR2-003).
//!
//! v1 is intentionally regex-based: heavyweight AST parsers (`syn`, `swc`,
//! `rustpython-parser`) would add cross-compile build-time and artifact-size
//! impact disproportionate to the v1 use case. False positives (e.g. `use`
//! lines inside string literals or comments) are accepted; ranking can
//! re-score in `#469`.
//!
//! Static `Regex` instances use `std::sync::OnceLock<Regex>` (Rust 1.70+)
//! rather than `once_cell::Lazy` because `once_cell` is not in
//! `Cargo.toml`. This keeps the v1 dependency delta at zero.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

/// Maximum bytes parsed per file. Files larger than this are skipped.
pub(crate) const MAX_REPO_GRAPH_FILE_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LangKind {
    Rust,
    Node,
    Python,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EdgeFragment {
    pub(crate) from_path: PathBuf,
    pub(crate) kind: FragmentKind,
    pub(crate) target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FragmentKind {
    /// `use foo::bar` / `import x` / `from y import z`
    Import,
    /// `fn name` / `class Name` / `struct Name` etc.
    Define,
}

fn rust_use() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\s*use\s+([\w:]+)").unwrap())
}
fn rust_define() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(fn|struct|enum|trait|mod)\s+(\w+)")
            .unwrap()
    })
}
fn node_import() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"^\s*import\b.*?from\s+["']([^"']+)["']"#).unwrap())
}
fn node_define() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\s*(?:export\s+(?:default\s+)?)?(?:async\s+)?(?:function|class|const|let|var)\s+(\w+)").unwrap())
}
fn py_import() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\s*(?:from\s+(\S+)\s+import|import\s+(\S+))").unwrap())
}
fn py_define() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\s*(?:def|class)\s+(\w+)").unwrap())
}

/// Detect the language from the file extension. Returns `None` for files we
/// don't parse (binary blobs, configs, lockfiles, etc.).
pub(crate) fn lang_from_path(path: &Path) -> Option<LangKind> {
    let ext = path.extension()?.to_str()?;
    match ext {
        "rs" => Some(LangKind::Rust),
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => Some(LangKind::Node),
        "py" => Some(LangKind::Python),
        _ => None,
    }
}

/// Read `path` (size-capped) and extract `EdgeFragment`s for the given
/// language. Returns an empty vec on any I/O error or oversized file — never
/// panics. The returned `from_path` is exactly the input `path` (caller is
/// responsible for ensuring it is workspace-relative).
pub(crate) fn parse_file(path: &Path, kind: LangKind) -> Vec<EdgeFragment> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };
    if meta.len() > MAX_REPO_GRAPH_FILE_BYTES {
        return Vec::new();
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&bytes);

    let mut out = Vec::new();
    for line in text.lines() {
        match kind {
            LangKind::Rust => {
                if let Some(c) = rust_use().captures(line) {
                    out.push(EdgeFragment {
                        from_path: path.to_path_buf(),
                        kind: FragmentKind::Import,
                        target: c.get(1).unwrap().as_str().to_string(),
                    });
                }
                if let Some(c) = rust_define().captures(line) {
                    out.push(EdgeFragment {
                        from_path: path.to_path_buf(),
                        kind: FragmentKind::Define,
                        target: c.get(2).unwrap().as_str().to_string(),
                    });
                }
            }
            LangKind::Node => {
                if let Some(c) = node_import().captures(line) {
                    out.push(EdgeFragment {
                        from_path: path.to_path_buf(),
                        kind: FragmentKind::Import,
                        target: c.get(1).unwrap().as_str().to_string(),
                    });
                }
                if let Some(c) = node_define().captures(line) {
                    out.push(EdgeFragment {
                        from_path: path.to_path_buf(),
                        kind: FragmentKind::Define,
                        target: c.get(1).unwrap().as_str().to_string(),
                    });
                }
            }
            LangKind::Python => {
                if let Some(c) = py_import().captures(line) {
                    let t = c.get(1).or_else(|| c.get(2)).unwrap().as_str();
                    out.push(EdgeFragment {
                        from_path: path.to_path_buf(),
                        kind: FragmentKind::Import,
                        target: t.to_string(),
                    });
                }
                if let Some(c) = py_define().captures(line) {
                    out.push(EdgeFragment {
                        from_path: path.to_path_buf(),
                        kind: FragmentKind::Define,
                        target: c.get(1).unwrap().as_str().to_string(),
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tmp(name: &str, content: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("anvil-rgp-{}-{}", std::process::id(), name));
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn rust_extracts_use_and_fn() {
        let p = write_tmp("a.rs", "use foo::bar;\npub fn hello() {}\nstruct S {}\n");
        let frags = parse_file(&p, LangKind::Rust);
        assert!(
            frags
                .iter()
                .any(|f| f.kind == FragmentKind::Import && f.target == "foo::bar")
        );
        assert!(
            frags
                .iter()
                .any(|f| f.kind == FragmentKind::Define && f.target == "hello")
        );
        assert!(
            frags
                .iter()
                .any(|f| f.kind == FragmentKind::Define && f.target == "S")
        );
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn node_extracts_import_and_export() {
        let p = write_tmp(
            "a.ts",
            "import x from './foo';\nexport function hello() {}\nexport const PI = 3;\n",
        );
        let frags = parse_file(&p, LangKind::Node);
        assert!(
            frags
                .iter()
                .any(|f| f.kind == FragmentKind::Import && f.target == "./foo")
        );
        assert!(
            frags
                .iter()
                .any(|f| f.kind == FragmentKind::Define && f.target == "hello")
        );
        assert!(
            frags
                .iter()
                .any(|f| f.kind == FragmentKind::Define && f.target == "PI")
        );
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn python_extracts_from_import_and_def() {
        let p = write_tmp(
            "a.py",
            "from foo import bar\nimport baz\ndef hello():\n    pass\nclass C:\n    pass\n",
        );
        let frags = parse_file(&p, LangKind::Python);
        assert!(
            frags
                .iter()
                .any(|f| f.kind == FragmentKind::Import && f.target == "foo")
        );
        assert!(
            frags
                .iter()
                .any(|f| f.kind == FragmentKind::Import && f.target == "baz")
        );
        assert!(
            frags
                .iter()
                .any(|f| f.kind == FragmentKind::Define && f.target == "hello")
        );
        assert!(
            frags
                .iter()
                .any(|f| f.kind == FragmentKind::Define && f.target == "C")
        );
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn skips_oversized_file() {
        let p = std::env::temp_dir().join(format!("anvil-rgp-big-{}", std::process::id()));
        // Write more than MAX_REPO_GRAPH_FILE_BYTES.
        let big = vec![b'x'; (MAX_REPO_GRAPH_FILE_BYTES as usize) + 1];
        std::fs::write(&p, big).unwrap();
        let frags = parse_file(&p, LangKind::Rust);
        assert!(frags.is_empty());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn missing_file_returns_empty() {
        let p = PathBuf::from("/nonexistent/anvil/repo_graph/file");
        assert!(parse_file(&p, LangKind::Rust).is_empty());
    }

    #[test]
    fn lang_from_path_recognizes_known_extensions() {
        assert_eq!(lang_from_path(Path::new("a.rs")), Some(LangKind::Rust));
        assert_eq!(lang_from_path(Path::new("a.ts")), Some(LangKind::Node));
        assert_eq!(lang_from_path(Path::new("a.py")), Some(LangKind::Python));
        assert_eq!(lang_from_path(Path::new("a.toml")), None);
    }
}
