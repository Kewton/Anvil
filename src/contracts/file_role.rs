//! File role classification for mutation telemetry (Issue #277).
//!
//! Classifies file paths into semantic roles to measure mutation order
//! and detect anti-patterns like peripheral-before-core editing.

/// Classifies a normalized cwd-relative path into a file role.
/// Absolute paths (starting with '/') are classified as "unknown" for safety.
pub(crate) fn classify_file_role(path: &str) -> &'static str {
    // Rule 0: Absolute path guard
    if path.starts_with('/') {
        return "unknown";
    }

    // Normalize: strip leading "./"
    let normalized = path.strip_prefix("./").unwrap_or(path);

    // Rule 1: test files
    if normalized.starts_with("tests/")
        || normalized.contains(".test.")
        || normalized.contains(".spec.")
    {
        return "test";
    }

    // Rule 2: docs or prompts
    if normalized.starts_with("docs/")
        || normalized.starts_with("prompts/")
        || normalized.ends_with(".md")
    {
        return "docs_or_prompt";
    }

    // Rule 3: config or schema
    if normalized.starts_with("config/")
        || normalized.ends_with(".json")
        || normalized.ends_with(".yaml")
        || normalized.ends_with(".yml")
        || normalized.ends_with(".toml")
        || path_contains_segment(normalized, "schema")
    {
        return "config_or_schema";
    }

    // Extract file stem (filename without extension) for rules 4-5
    let file_stem = std::path::Path::new(normalized)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Rule 4: route/handler/controller
    if contains_word_in_stem(&file_stem, &["route", "handler", "controller"]) {
        return "route_or_handler";
    }

    // Rule 5: adapter/wrapper/client
    if contains_word_in_stem(&file_stem, &["adapter", "wrapper", "client"]) {
        return "adapter_or_wrapper";
    }

    // Rule 6: core implementation source files
    let core_extensions = [
        ".rs", ".ts", ".js", ".py", ".go", ".java", ".c", ".cpp", ".rb",
    ];
    if core_extensions.iter().any(|ext| normalized.ends_with(ext)) {
        return "core_impl";
    }

    // Rule 7: fallback
    "unknown"
}

/// Converts an absolute path to a cwd-relative path.
/// - Strips the cwd prefix to produce a relative path
/// - Normalizes "./" prefix
/// - Returns abs_path as-is if outside cwd (will be classified as "unknown" by classify_file_role)
pub(crate) fn make_cwd_relative(abs_path: &str, cwd: &std::path::Path) -> String {
    let abs = std::path::Path::new(abs_path);
    if let Ok(rel) = abs.strip_prefix(cwd) {
        let rel_str = rel.to_string_lossy().to_string();
        if rel_str.is_empty() {
            return ".".to_string();
        }
        rel_str
    } else {
        abs_path.to_string()
    }
}

/// Check if a path contains a specific segment (between '/' separators).
fn path_contains_segment(path: &str, segment: &str) -> bool {
    path.split('/').any(|s| s == segment)
}

/// Check if the file stem contains any of the given words.
fn contains_word_in_stem(stem: &str, words: &[&str]) -> bool {
    let lower = stem.to_lowercase();
    words.iter().any(|w| lower.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Rule 0: Absolute path guard ---
    #[test]
    fn absolute_path_returns_unknown() {
        assert_eq!(classify_file_role("/usr/local/bin/foo.rs"), "unknown");
        assert_eq!(
            classify_file_role("/home/user/project/src/main.rs"),
            "unknown"
        );
    }

    // --- Rule 1: test files ---
    #[test]
    fn tests_dir_classified_as_test() {
        assert_eq!(classify_file_role("tests/foo.rs"), "test");
        assert_eq!(classify_file_role("tests/integration/bar.rs"), "test");
    }

    #[test]
    fn dot_test_classified_as_test() {
        assert_eq!(classify_file_role("src/foo.test.ts"), "test");
    }

    #[test]
    fn dot_spec_classified_as_test() {
        assert_eq!(classify_file_role("src/bar.spec.js"), "test");
    }

    // --- Rule 2: docs or prompts ---
    #[test]
    fn docs_dir_classified_as_docs() {
        assert_eq!(classify_file_role("docs/guide.md"), "docs_or_prompt");
    }

    #[test]
    fn prompts_dir_classified_as_docs() {
        assert_eq!(classify_file_role("prompts/system.txt"), "docs_or_prompt");
    }

    #[test]
    fn md_extension_classified_as_docs() {
        assert_eq!(classify_file_role("README.md"), "docs_or_prompt");
        assert_eq!(classify_file_role("CHANGELOG.md"), "docs_or_prompt");
    }

    // --- Rule 3: config or schema ---
    #[test]
    fn config_dir_classified_as_config() {
        assert_eq!(classify_file_role("config/settings.rs"), "config_or_schema");
    }

    #[test]
    fn json_extension_classified_as_config() {
        assert_eq!(classify_file_role("package.json"), "config_or_schema");
    }

    #[test]
    fn yaml_extension_classified_as_config() {
        assert_eq!(
            classify_file_role("docker-compose.yaml"),
            "config_or_schema"
        );
        assert_eq!(classify_file_role("config.yml"), "config_or_schema");
    }

    #[test]
    fn toml_extension_classified_as_config() {
        assert_eq!(classify_file_role("Cargo.toml"), "config_or_schema");
    }

    #[test]
    fn schema_segment_classified_as_config() {
        assert_eq!(classify_file_role("src/schema/user.rs"), "config_or_schema");
    }

    // --- Rule 4: route/handler/controller ---
    #[test]
    fn route_handler_controller_classified() {
        assert_eq!(classify_file_role("src/api/route.rs"), "route_or_handler");
        assert_eq!(classify_file_role("src/handler.py"), "route_or_handler");
        assert_eq!(
            classify_file_role("src/user_controller.rb"),
            "route_or_handler"
        );
    }

    // --- Rule 5: adapter/wrapper/client ---
    #[test]
    fn adapter_wrapper_client_classified() {
        assert_eq!(
            classify_file_role("src/http_adapter.rs"),
            "adapter_or_wrapper"
        );
        assert_eq!(
            classify_file_role("src/api_wrapper.ts"),
            "adapter_or_wrapper"
        );
        assert_eq!(
            classify_file_role("src/redis_client.py"),
            "adapter_or_wrapper"
        );
    }

    // --- Rule 6: core implementation ---
    #[test]
    fn core_impl_extensions() {
        assert_eq!(classify_file_role("src/main.rs"), "core_impl");
        assert_eq!(classify_file_role("src/app.ts"), "core_impl");
        assert_eq!(classify_file_role("src/utils.js"), "core_impl");
        assert_eq!(classify_file_role("src/lib.py"), "core_impl");
        assert_eq!(classify_file_role("src/main.go"), "core_impl");
        assert_eq!(classify_file_role("src/App.java"), "core_impl");
        assert_eq!(classify_file_role("src/helper.c"), "core_impl");
        assert_eq!(classify_file_role("src/engine.cpp"), "core_impl");
        assert_eq!(classify_file_role("src/util.rb"), "core_impl");
    }

    // --- Rule 7: unknown ---
    #[test]
    fn unknown_fallback() {
        assert_eq!(classify_file_role("Makefile"), "unknown");
        assert_eq!(classify_file_role("LICENSE"), "unknown");
        assert_eq!(classify_file_role("data/output.bin"), "unknown");
    }

    // --- Anvil real file paths (smoke tests) ---
    #[test]
    fn anvil_real_paths() {
        assert_eq!(classify_file_role("src/provider/ollama.rs"), "core_impl");
        assert_eq!(classify_file_role("tests/telemetry_artifact.rs"), "test");
        assert_eq!(classify_file_role("src/contracts/mod.rs"), "core_impl");
        assert_eq!(classify_file_role("src/app/agentic.rs"), "core_impl");
        assert_eq!(classify_file_role("src/config/mod.rs"), "core_impl");
    }

    // --- "./" normalization ---
    #[test]
    fn dot_slash_normalization() {
        assert_eq!(classify_file_role("./src/x.rs"), "core_impl");
        assert_eq!(classify_file_role("./tests/foo.rs"), "test");
    }

    // --- make_cwd_relative ---
    #[test]
    fn make_cwd_relative_strips_prefix() {
        let cwd = std::path::Path::new("/home/user/project");
        assert_eq!(
            make_cwd_relative("/home/user/project/src/main.rs", cwd),
            "src/main.rs"
        );
    }

    #[test]
    fn make_cwd_relative_outside_cwd() {
        let cwd = std::path::Path::new("/home/user/project");
        assert_eq!(
            make_cwd_relative("/other/path/foo.rs", cwd),
            "/other/path/foo.rs"
        );
    }

    #[test]
    fn make_cwd_relative_exact_cwd() {
        let cwd = std::path::Path::new("/home/user/project");
        assert_eq!(make_cwd_relative("/home/user/project", cwd), ".");
    }
}
