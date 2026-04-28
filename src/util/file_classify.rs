//! Single source of truth for "is this path a test/setup/implementation file?"
//! style classification (Issue #456 / DR1-007).
//!
//! Both `RepoVerification` (in `agent/orchestration.rs`) and AnvilScore
//! computation (in `session/anvil_score.rs`) depend on this classification.
//! Keeping the helpers in `util` avoids pulling `agent` into `session` (a
//! layer inversion) and lets future callers re-use the same definitions.
//!
//! The helpers operate on `&Path` rather than `&str` so callers don't have to
//! round-trip through `display()`. Each helper is pure / deterministic.

use std::path::Path;

/// Heuristic detection for test files. Mirrors the convention already used by
/// `verify_repo_progress` (matches `__tests__`, `.test.`, `.spec.` substrings
/// in the path). The check is intentionally substring-based to catch nested
/// `__tests__/` dirs and dotted file naming conventions across JS / TS /
/// Python ecosystems.
pub fn is_test_file(path: &Path) -> bool {
    let display = path.display().to_string();
    display.contains("__tests__") || display.contains(".test.") || display.contains(".spec.")
}

/// Setup / configuration files that ship with the repo. The list is pinned to
/// the same set previously hard-coded in `agent::orchestration::is_setup_file`
/// — extending it requires updating both the helper and the AC tests that
/// rely on the classification.
pub fn is_setup_file(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    matches!(
        file_name,
        "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "tsconfig.json"
            | "jest.config.js"
            | "jest.config.ts"
            | "vitest.config.ts"
            | "vitest.config.js"
            | "next.config.ts"
            | "next.config.js"
            | "eslint.config.js"
            | "eslint.config.mjs"
    )
}

/// Implementation source files. Recognises a small fixed set of language /
/// stylesheet / template extensions. Test and setup files are not removed
/// here; callers are expected to apply `is_test_file` / `is_setup_file` first
/// (the order used in `verify_repo_progress`).
pub fn is_implementation_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some(
            "rs" | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "py"
                | "go"
                | "java"
                | "kt"
                | "swift"
                | "c"
                | "cc"
                | "cpp"
                | "h"
                | "hpp"
                | "css"
                | "scss"
                | "html"
                | "mdx"
                | "vue"
                | "svelte"
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_is_test_file_double_underscore_dir() {
        assert!(is_test_file(&PathBuf::from("src/__tests__/foo.ts")));
        assert!(is_test_file(&PathBuf::from("__tests__/foo.tsx")));
    }

    #[test]
    fn test_is_test_file_dotted_naming() {
        assert!(is_test_file(&PathBuf::from("src/foo.test.ts")));
        assert!(is_test_file(&PathBuf::from("packages/lib/bar.spec.js")));
    }

    #[test]
    fn test_is_test_file_negative() {
        assert!(!is_test_file(&PathBuf::from("src/main.rs")));
        assert!(!is_test_file(&PathBuf::from("README.md")));
        // Non-dotted "tests" is intentionally NOT matched (matches Rust idiom
        // of `tests/` integration dir at workspace root, but we leave that to
        // higher-level rules).
        assert!(!is_test_file(&PathBuf::from("tests/foo.rs")));
    }

    #[test]
    fn test_is_setup_file_known_names() {
        for name in [
            "package.json",
            "package-lock.json",
            "pnpm-lock.yaml",
            "yarn.lock",
            "tsconfig.json",
            "jest.config.js",
            "jest.config.ts",
            "vitest.config.ts",
            "vitest.config.js",
            "next.config.ts",
            "next.config.js",
            "eslint.config.js",
            "eslint.config.mjs",
        ] {
            assert!(
                is_setup_file(&PathBuf::from(name)),
                "expected {name} to classify as setup"
            );
        }
    }

    #[test]
    fn test_is_setup_file_negative() {
        assert!(!is_setup_file(&PathBuf::from("Cargo.toml")));
        assert!(!is_setup_file(&PathBuf::from("src/main.rs")));
        assert!(!is_setup_file(&PathBuf::from("README.md")));
    }

    #[test]
    fn test_is_implementation_file_extensions() {
        for ext in [
            "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "kt", "swift", "c", "cc", "cpp",
            "h", "hpp", "css", "scss", "html", "mdx", "vue", "svelte",
        ] {
            let path = PathBuf::from(format!("foo.{ext}"));
            assert!(
                is_implementation_file(&path),
                "expected .{ext} to classify as impl"
            );
        }
    }

    #[test]
    fn test_is_implementation_file_negative() {
        assert!(!is_implementation_file(&PathBuf::from("README.md")));
        assert!(!is_implementation_file(&PathBuf::from("Cargo.toml")));
        assert!(!is_implementation_file(&PathBuf::from("foo")));
        assert!(!is_implementation_file(&PathBuf::from("foo.txt")));
    }

    #[test]
    fn test_no_panic_on_no_extension() {
        let path = PathBuf::from("Makefile");
        assert!(!is_implementation_file(&path));
        assert!(!is_setup_file(&path));
        assert!(!is_test_file(&path));
    }
}
