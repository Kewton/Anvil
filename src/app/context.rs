//! @file reference parser and context injection utilities.
//!
//! Detects `@path/to/file` references in user input and provides
//! utilities for expanding them into file contents.

use regex::Regex;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use crate::tooling::diff::is_binary_content;
use crate::tooling::resolve_sandbox_path;

/// A parsed @file reference from user input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AtReference {
    /// Start position in the original input (index of '@').
    pub start: usize,
    /// End position in the original input (exclusive).
    pub end: usize,
    /// The file path (without the leading '@').
    pub path: String,
}

/// Regex pattern for @file references.
/// Matches: `@` preceded by start-of-string or whitespace, followed by
/// a path containing `/` (either `./` prefix or `word/` prefix).
/// Excludes: email addresses, SNS handles, annotations, root-level files.
static AT_REFERENCE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[ \t])@((?:\./|[a-zA-Z0-9_-]+/)[^ \t]+)").expect("invalid regex")
});

/// Parse @file references from user input.
///
/// Returns a list of `AtReference` with start/end positions pointing to
/// the `@path` portion (not including any leading whitespace).
pub(crate) fn parse_at_references(input: &str) -> Vec<AtReference> {
    let mut refs = Vec::new();
    for cap in AT_REFERENCE_RE.captures_iter(input) {
        let full_match = cap.get(0).unwrap();
        let path_match = cap.get(1).unwrap();
        // The '@' is right before the captured path group.
        // full_match may include a leading space, so we compute '@' position
        // from the path match start minus 1.
        let at_pos = path_match.start() - 1;
        let end = full_match.end();
        refs.push(AtReference {
            start: at_pos,
            end,
            path: path_match.as_str().to_string(),
        });
    }
    refs
}

/// Sensitive file patterns that should never be expanded via @file.
/// Prevents accidental leakage of secrets to LLM backends.
const SENSITIVE_FILE_PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
    "credentials.json",
    "secrets.*",
    "*.secret",
    "*.secrets",
    ".netrc",
    ".npmrc",
    ".pypirc",
    "token.json",
    "service-account*.json",
];

/// Check whether a file path matches the sensitive file blocklist.
///
/// Matching is performed against the file name (basename) only, using
/// simple string operations: exact match, prefix (`starts_with`),
/// suffix (`ends_with`), and prefix+suffix for patterns like `service-account*.json`.
fn is_sensitive_file(path: &str) -> bool {
    let file_name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if file_name.is_empty() {
        return false;
    }
    for pattern in SENSITIVE_FILE_PATTERNS {
        if let Some(suffix) = pattern.strip_prefix('*') {
            // e.g. "*.pem" → ends_with(".pem")
            if file_name.ends_with(suffix) {
                return true;
            }
        } else if let Some(prefix) = pattern.strip_suffix('*') {
            // e.g. ".env.*" → starts_with(".env.")
            if file_name.starts_with(prefix) {
                return true;
            }
        } else if pattern.contains('*') {
            // e.g. "service-account*.json" → starts_with("service-account") && ends_with(".json")
            let parts: Vec<&str> = pattern.splitn(2, '*').collect();
            if parts.len() == 2
                && file_name.starts_with(parts[0])
                && file_name.ends_with(parts[1])
                && file_name.len() >= parts[0].len() + parts[1].len()
            {
                return true;
            }
        } else {
            // exact match
            if file_name == *pattern {
                return true;
            }
        }
    }
    false
}

/// Resolve a single @file reference, returning the file content as text.
///
/// Steps: sensitive check → sandbox validation → existence/size check →
/// read → TOCTOU double-check → binary check → UTF-8 conversion.
///
/// Error messages use the relative path (`at_ref.path`) only, never the
/// resolved absolute path (SEC-006).
fn resolve_at_reference(
    at_ref: &AtReference,
    sandbox_root: &Path,
    max_file_size: u64,
) -> Result<String, String> {
    // 0. Sensitive file blocklist (SEC-007)
    if is_sensitive_file(&at_ref.path) {
        return Err(format!(
            "機密ファイルの展開は許可されていません: {}（LLMに送信される可能性があるため）",
            at_ref.path
        ));
    }

    // 1. Sandbox validation (IC-007: map ToolRuntimeError to String)
    let resolved = resolve_sandbox_path(sandbox_root, &at_ref.path).map_err(|_| {
        format!(
            "サンドボックス外のファイルにはアクセスできません: {}",
            at_ref.path
        )
    })?;

    // 2. File existence and size check (SEC-006: relative paths in errors)
    let metadata = fs::metadata(&resolved)
        .map_err(|_| format!("ファイルが見つかりません: {}", at_ref.path))?;
    if metadata.len() > max_file_size {
        return Err(format!(
            "ファイルサイズ上限超過: {} ({}bytes > {}bytes)",
            at_ref.path,
            metadata.len(),
            max_file_size
        ));
    }

    // 3. Read file content
    let content = fs::read(&resolved)
        .map_err(|_| format!("ファイルの読み込みに失敗しました: {}", at_ref.path))?;

    // 4. TOCTOU double-check: re-verify path after read (SEC-003).
    //    resolve_sandbox_path() skips canonicalize() for non-existent paths,
    //    so this post-read canonicalize is a mandatory defence layer.
    let canonical_after = fs::canonicalize(&resolved)
        .map_err(|_| format!("パスの検証に失敗しました: {}", at_ref.path))?;
    let root_canonical = fs::canonicalize(sandbox_root)
        .map_err(|_| "サンドボックスルートの検証に失敗しました".to_string())?;
    if !canonical_after.starts_with(&root_canonical) {
        return Err("サンドボックス外へのアクセスが検出されました".into());
    }

    // 5. Binary detection
    if is_binary_content(&content) {
        return Err(format!("バイナリファイルは展開できません: {}", at_ref.path));
    }

    // 6. UTF-8 conversion
    String::from_utf8(content)
        .map_err(|_| format!("テキストファイルとして読み取れません: {}", at_ref.path))
}

/// Maximum number of @file references per message (SEC-004).
const MAX_AT_FILE_REFS: usize = 10;
/// Maximum total expanded bytes per message (SEC-004).
const MAX_TOTAL_EXPANDED_BYTES: usize = 512_000; // 500KB

/// Expand @file references in user input, replacing each with the file content.
///
/// Returns `(Some(expanded_text), errors)` when at least one reference was
/// successfully expanded, or `(None, errors)` when no expansion occurred.
/// References that fail to resolve are left as-is in the text.
pub(crate) fn expand_at_references(
    input: &str,
    sandbox_root: &Path,
    max_file_size: u64,
) -> (Option<String>, Vec<String>) {
    let refs = parse_at_references(input);
    if refs.is_empty() {
        return (None, vec![]);
    }

    let mut errors: Vec<String> = vec![];

    // SEC-004: reference count limit
    if refs.len() > MAX_AT_FILE_REFS {
        errors.push(format!(
            "@file参照数が上限を超えています（{} > {}）。先頭{}件のみ展開します",
            refs.len(),
            MAX_AT_FILE_REFS,
            MAX_AT_FILE_REFS
        ));
    }
    let refs_to_process = &refs[..refs.len().min(MAX_AT_FILE_REFS)];

    let mut expanded = input.to_string();
    let mut any_expanded = false;
    let mut total_expanded_bytes: usize = 0;

    // Iterate in reverse to prevent position shift during replacement
    for at_ref in refs_to_process.iter().rev() {
        match resolve_at_reference(at_ref, sandbox_root, max_file_size) {
            Ok(text) => {
                // SEC-004: total size limit
                if total_expanded_bytes + text.len() > MAX_TOTAL_EXPANDED_BYTES {
                    errors.push(format!(
                        "{}: 展開後の合計サイズが上限({}KB)を超えるためスキップ",
                        at_ref.path,
                        MAX_TOTAL_EXPANDED_BYTES / 1024
                    ));
                    continue;
                }
                total_expanded_bytes += text.len();

                let replacement = format!("@{}\n```\n{}\n```", at_ref.path, text);
                expanded.replace_range(at_ref.start..at_ref.end, &replacement);
                any_expanded = true;
            }
            Err(e) => {
                errors.push(format!("{}: {}", at_ref.path, e));
            }
        }
    }

    let result = if any_expanded { Some(expanded) } else { None };
    (result, errors)
}

/// Format the current date and timezone for system prompt injection.
///
/// Called per-turn by `build_dynamic_system_prompt()` so the date stays
/// fresh even in long-running sessions.
pub(crate) fn format_date_prompt() -> String {
    use chrono::Local;

    let now = Local::now();
    let date_str = now.format("%Y-%m-%d (%a)").to_string();
    let tz_offset = now.format("%:z").to_string();

    format!(
        "\n\n## Current date\nToday is {date_str}. Time zone: {tz_offset}.\n\
         If the user asks for the current time or other runtime-local facts, \
         use `shell.exec` to verify.\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs as stdfs;

    #[test]
    fn parse_single_path_reference() {
        let refs = parse_at_references("@src/main.rs");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].path, "src/main.rs");
        assert_eq!(refs[0].start, 0);
        assert_eq!(refs[0].end, 12);
    }

    #[test]
    fn parse_dotslash_reference() {
        let refs = parse_at_references("@./config.toml");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].path, "./config.toml");
    }

    #[test]
    fn parse_multiple_references() {
        let refs = parse_at_references("@src/a.rs @src/b.rs");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].path, "src/a.rs");
        assert_eq!(refs[1].path, "src/b.rs");
    }

    #[test]
    fn ignore_username_handle() {
        let refs = parse_at_references("@username");
        assert!(refs.is_empty());
    }

    #[test]
    fn ignore_email_address() {
        let refs = parse_at_references("user@example.com");
        assert!(refs.is_empty());
    }

    #[test]
    fn ignore_annotation() {
        let refs = parse_at_references("@Override");
        assert!(refs.is_empty());
    }

    #[test]
    fn ignore_root_level_file_without_path_separator() {
        let refs = parse_at_references("@Cargo.toml");
        assert!(refs.is_empty());
    }

    #[test]
    fn parse_reference_mixed_in_text() {
        let input = "Please review @src/main.rs and fix the bug";
        let refs = parse_at_references(input);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].path, "src/main.rs");
        assert_eq!(refs[0].start, 14);
        assert_eq!(refs[0].end, 26);
        assert_eq!(&input[refs[0].start..refs[0].end], "@src/main.rs");
    }

    #[test]
    fn parse_reference_at_line_start() {
        let refs = parse_at_references("@src/lib.rs is the main module");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].path, "src/lib.rs");
        assert_eq!(refs[0].start, 0);
    }

    #[test]
    fn effective_content_returns_expanded_when_set() {
        let mut msg = crate::session::SessionMessage::new(
            crate::session::MessageRole::User,
            "you",
            "raw input",
        );
        msg.expanded_content = Some("expanded input".to_string());
        assert_eq!(msg.effective_content(), "expanded input");
    }

    #[test]
    fn effective_content_returns_content_when_none() {
        let msg = crate::session::SessionMessage::new(
            crate::session::MessageRole::User,
            "you",
            "raw input",
        );
        assert_eq!(msg.effective_content(), "raw input");
    }

    #[test]
    fn format_date_prompt_contains_date_and_timezone() {
        let prompt = super::format_date_prompt();
        assert!(prompt.contains("Today is "));
        assert!(prompt.contains("Time zone: "));
        assert!(prompt.contains("shell.exec"));
        assert!(prompt.contains("## Current date"));
    }

    // ---- Task 2.1: is_sensitive_file tests ----

    #[test]
    fn test_is_sensitive_file_env() {
        assert!(is_sensitive_file(".env"));
        assert!(is_sensitive_file(".env.local"));
        assert!(is_sensitive_file(".env.production"));
        assert!(is_sensitive_file("subdir/.env"));
        assert!(is_sensitive_file("subdir/.env.local"));
    }

    #[test]
    fn test_is_sensitive_file_keys() {
        assert!(is_sensitive_file("server.pem"));
        assert!(is_sensitive_file("private.key"));
        assert!(is_sensitive_file("cert.p12"));
        assert!(is_sensitive_file("cert.pfx"));
        assert!(is_sensitive_file("id_rsa"));
        assert!(is_sensitive_file("id_ed25519"));
        assert!(is_sensitive_file("id_ecdsa"));
        assert!(is_sensitive_file("id_dsa"));
        assert!(is_sensitive_file("credentials.json"));
        assert!(is_sensitive_file("secrets.yaml"));
        assert!(is_sensitive_file("db.secret"));
        assert!(is_sensitive_file("app.secrets"));
        assert!(is_sensitive_file(".netrc"));
        assert!(is_sensitive_file(".npmrc"));
        assert!(is_sensitive_file(".pypirc"));
        assert!(is_sensitive_file("token.json"));
        assert!(is_sensitive_file("service-account-prod.json"));
    }

    #[test]
    fn test_is_sensitive_file_normal() {
        assert!(!is_sensitive_file("src/main.rs"));
        assert!(!is_sensitive_file("Cargo.toml"));
        assert!(!is_sensitive_file("README.md"));
        assert!(!is_sensitive_file("config.toml"));
        assert!(!is_sensitive_file("src/lib.rs"));
        assert!(!is_sensitive_file("test.json"));
    }

    // ---- Task 2.2: resolve_at_reference tests ----

    /// Helper: create a temp dir with a subdirectory and a file inside it.
    fn setup_sandbox() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let sub = tmp.path().join("src");
        stdfs::create_dir_all(&sub).unwrap();
        (tmp, sub)
    }

    #[test]
    fn test_resolve_at_reference_success() {
        let (tmp, sub) = setup_sandbox();
        let file_path = sub.join("hello.txt");
        stdfs::write(&file_path, "hello world").unwrap();

        let at_ref = AtReference {
            start: 0,
            end: 14,
            path: "src/hello.txt".to_string(),
        };
        let result = resolve_at_reference(&at_ref, tmp.path(), 102_400);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "hello world");
    }

    #[test]
    fn test_resolve_at_reference_not_found() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let at_ref = AtReference {
            start: 0,
            end: 20,
            path: "src/nonexistent.txt".to_string(),
        };
        let result = resolve_at_reference(&at_ref, tmp.path(), 102_400);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("ファイルが見つかりません"), "got: {err}");
    }

    #[test]
    fn test_resolve_at_reference_sandbox_violation() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let at_ref = AtReference {
            start: 0,
            end: 12,
            path: "../outside".to_string(),
        };
        let result = resolve_at_reference(&at_ref, tmp.path(), 102_400);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("サンドボックス外"), "got: {err}");
    }

    #[test]
    fn test_resolve_at_reference_binary() {
        let (tmp, sub) = setup_sandbox();
        let file_path = sub.join("binary.bin");
        // Write content with NUL bytes
        stdfs::write(&file_path, b"hello\x00world").unwrap();

        let at_ref = AtReference {
            start: 0,
            end: 16,
            path: "src/binary.bin".to_string(),
        };
        let result = resolve_at_reference(&at_ref, tmp.path(), 102_400);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("バイナリファイル"), "got: {err}");
    }

    #[test]
    fn test_resolve_at_reference_sensitive() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let env_dir = tmp.path().join("config");
        stdfs::create_dir_all(&env_dir).unwrap();
        stdfs::write(env_dir.join(".env"), "SECRET=abc").unwrap();

        let at_ref = AtReference {
            start: 0,
            end: 12,
            path: "config/.env".to_string(),
        };
        let result = resolve_at_reference(&at_ref, tmp.path(), 102_400);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("機密ファイル"), "got: {err}");
    }

    #[test]
    fn test_resolve_at_reference_size_limit() {
        let (tmp, sub) = setup_sandbox();
        let file_path = sub.join("big.txt");
        // Create a file larger than the limit (use 200 bytes, set limit to 100)
        stdfs::write(&file_path, "x".repeat(200)).unwrap();

        let at_ref = AtReference {
            start: 0,
            end: 12,
            path: "src/big.txt".to_string(),
        };
        let result = resolve_at_reference(&at_ref, tmp.path(), 100);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("ファイルサイズ上限超過"), "got: {err}");
    }

    // ---- Task 2.3: expand_at_references tests ----

    #[test]
    fn test_expand_at_references_basic() {
        let (tmp, sub) = setup_sandbox();
        stdfs::write(sub.join("main.rs"), "fn main() {}").unwrap();

        let input = "Review @src/main.rs please";
        let (expanded, errors) = expand_at_references(input, tmp.path(), 102_400);
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert!(expanded.is_some());
        let text = expanded.unwrap();
        assert!(text.contains("```\nfn main() {}\n```"), "got: {text}");
        assert!(text.contains("@src/main.rs"));
        assert!(text.contains("please"));
    }

    #[test]
    fn test_expand_at_references_partial() {
        let (tmp, sub) = setup_sandbox();
        stdfs::write(sub.join("good.rs"), "good content").unwrap();
        // bad.rs does not exist

        let input = "@src/good.rs @src/bad.rs";
        let (expanded, errors) = expand_at_references(input, tmp.path(), 102_400);
        // One error for the missing file
        assert_eq!(errors.len(), 1, "errors: {errors:?}");
        assert!(errors[0].contains("bad.rs"));
        // Should still expand the good file
        assert!(expanded.is_some());
        let text = expanded.unwrap();
        assert!(text.contains("good content"));
        // The bad reference should remain as-is
        assert!(text.contains("@src/bad.rs"));
    }

    #[test]
    fn test_expand_at_references_no_refs() {
        let tmp = tempfile::tempdir().unwrap();
        let (expanded, errors) = expand_at_references("no refs here", tmp.path(), 102_400);
        assert!(expanded.is_none());
        assert!(errors.is_empty());
    }

    #[test]
    fn test_expand_at_references_max_refs() {
        let (tmp, sub) = setup_sandbox();
        // Create 11 files
        for i in 0..11 {
            stdfs::write(sub.join(format!("f{i}.rs")), format!("content{i}")).unwrap();
        }

        // Build input with 11 @file references
        let input: String = (0..11)
            .map(|i| format!("@src/f{i}.rs"))
            .collect::<Vec<_>>()
            .join(" ");

        let (expanded, errors) = expand_at_references(&input, tmp.path(), 102_400);
        // Should have an error about exceeding limit
        assert!(
            errors.iter().any(|e| e.contains("上限を超えています")),
            "errors: {errors:?}"
        );
        // Should still expand (up to 10)
        assert!(expanded.is_some());
    }
}
