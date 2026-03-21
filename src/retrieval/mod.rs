use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fmt::{Display, Formatter};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

/// Current schema version for cache compatibility.
const CURRENT_SCHEMA_VERSION: u32 = 2;

/// Maximum file size for on-demand content reading (1 MB).
const MAX_CONTENT_SIZE: u64 = 1_048_576;

/// 1st pass candidate count.
const FIRST_PASS_CANDIDATES: usize = 50;

/// Default search result limit (public for use by app layer).
pub const DEFAULT_SEARCH_LIMIT: usize = 10;

/// Query input guards (DoS mitigation).
const MAX_QUERY_BYTES: usize = 500;
const MAX_KEYWORDS: usize = 20;

/// Score constants — 1st pass (path only).
const SCORE_FILENAME_EXACT: i32 = 200;
const SCORE_FILENAME_PARTIAL: i32 = 120;
const SCORE_PATH_PARTIAL: i32 = 80;
const SCORE_CHANGED_FILE_BOOST: i32 = 100;
const SCORE_RELATED_TEST_BOOST: i32 = 60;
const SCORE_NEIGHBOR_BOOST: i32 = 50;

/// Score constants — 2nd pass (content).
const SCORE_SYMBOL_MATCH: i32 = 40;
const SCORE_CONTENT_LINE: i32 = 8;
const SCORE_CONTENT_LINE_CAP: i32 = 80;
const SCORE_ALL_KEYWORDS_BONUS: i32 = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalMatch {
    pub path: String,
    pub score: i32,
    pub snippets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievalResult {
    pub query: String,
    pub matches: Vec<RetrievalMatch>,
}

#[derive(Debug)]
pub enum RetrievalError {
    Walk(std::io::Error),
    CacheRead(std::io::Error),
    CacheWrite(std::io::Error),
    CacheDecode(serde_json::Error),
    CacheEncode(serde_json::Error),
}

impl Display for RetrievalError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Walk(err) => write!(f, "failed to index repository: {err}"),
            Self::CacheRead(err) => write!(f, "failed to read retrieval cache: {err}"),
            Self::CacheWrite(err) => write!(f, "failed to write retrieval cache: {err}"),
            Self::CacheDecode(err) => write!(f, "invalid retrieval cache json: {err}"),
            Self::CacheEncode(err) => write!(f, "failed to encode retrieval cache: {err}"),
        }
    }
}

impl std::error::Error for RetrievalError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct IndexedFile {
    relative_path: String,
    size_bytes: u64,
    modified_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryIndex {
    #[serde(default)]
    schema_version: u32,
    #[serde(skip)]
    root: PathBuf,
    #[serde(default)]
    manifest_hash: u64,
    files: Vec<IndexedFile>,
}

impl PartialEq for RepositoryIndex {
    fn eq(&self, other: &Self) -> bool {
        self.schema_version == other.schema_version
            && self.manifest_hash == other.manifest_hash
            && self.files == other.files
    }
}

impl Eq for RepositoryIndex {}

impl RepositoryIndex {
    pub fn build(root: &Path) -> Result<Self, RetrievalError> {
        let mut files = Vec::new();
        collect_files(root, &mut files);
        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        let manifest_hash = compute_manifest_hash(&files);
        Ok(Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            root: root.to_path_buf(),
            manifest_hash,
            files,
        })
    }

    pub fn load_or_build(root: &Path, cache_path: &Path) -> Result<Self, RetrievalError> {
        if cache_path.exists() {
            let bytes = fs::read(cache_path).map_err(RetrievalError::CacheRead)?;
            match serde_json::from_slice::<RepositoryIndex>(&bytes) {
                Ok(mut index) => {
                    if index.schema_version < CURRENT_SCHEMA_VERSION {
                        // Old schema version — auto-rebuild
                        let new_index = Self::build(root)?;
                        new_index.save(cache_path)?;
                        return Ok(new_index);
                    }
                    index.root = root.to_path_buf();
                    if index.is_current_for(root)? {
                        return Ok(index);
                    }
                }
                Err(_) => {
                    // Deserialization failure — auto-rebuild (no crash)
                }
            }
        }

        let index = Self::build(root)?;
        index.save(cache_path)?;
        Ok(index)
    }

    pub fn search(&self, query: &str, limit: usize) -> RetrievalResult {
        let keywords = split_keywords(query);
        if keywords.is_empty() {
            return RetrievalResult {
                query: query.to_string(),
                matches: Vec::new(),
            };
        }

        // Boost context (git diff, related tests) — computed once per search
        let changed_files = get_changed_files(&self.root);
        let related_tests = find_related_tests(&self.files, &changed_files);
        let neighbor_dirs = get_neighbor_dirs(&changed_files);

        // 1st pass: path/name scoring only (no I/O)
        let mut scored: Vec<_> = self
            .files
            .iter()
            .filter_map(|f| {
                score_file_path_only(f, &keywords, &changed_files, &related_tests, &neighbor_dirs)
            })
            .collect();
        scored.sort_by(|a, b| b.score.cmp(&a.score));
        scored.truncate(FIRST_PASS_CANDIDATES);

        let mut candidates = scored;

        // Wildcard: changed_files + related_tests always pass to 2nd pass
        for f in &self.files {
            let dominated = changed_files.contains(&f.relative_path)
                || related_tests.contains(&f.relative_path);
            if dominated && !candidates.iter().any(|c| c.path == f.relative_path) {
                candidates.push(RetrievalMatch {
                    path: f.relative_path.clone(),
                    score: 0,
                    snippets: vec![],
                });
            }
        }

        // Fill remaining slots with unscored files for content-only matching
        if candidates.len() < FIRST_PASS_CANDIDATES {
            let remaining = FIRST_PASS_CANDIDATES - candidates.len();
            for f in &self.files {
                if candidates.len() >= FIRST_PASS_CANDIDATES + remaining {
                    break;
                }
                if !candidates.iter().any(|c| c.path == f.relative_path) {
                    candidates.push(RetrievalMatch {
                        path: f.relative_path.clone(),
                        score: 0,
                        snippets: vec![],
                    });
                }
                if candidates.len() >= FIRST_PASS_CANDIDATES {
                    break;
                }
            }
        }

        // 2nd pass: read_file_content + score_content (I/O separated from scoring)
        let mut matches: Vec<_> = candidates
            .into_iter()
            .map(|c| {
                let content = read_file_content(&self.root, &c.path);
                score_content(c, content.as_deref(), &keywords)
            })
            .collect();
        matches.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
        matches.truncate(limit);

        RetrievalResult {
            query: query.to_string(),
            matches,
        }
    }

    pub fn save(&self, cache_path: &Path) -> Result<(), RetrievalError> {
        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent).map_err(RetrievalError::CacheWrite)?;
        }
        let bytes = serde_json::to_vec(self).map_err(RetrievalError::CacheEncode)?;
        fs::write(cache_path, bytes).map_err(RetrievalError::CacheWrite)
    }

    fn is_current_for(&self, root: &Path) -> Result<bool, RetrievalError> {
        let mut current = Vec::new();
        collect_files(root, &mut current);
        current.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        let current_hash = compute_manifest_hash(&current);
        if current_hash != self.manifest_hash {
            return Ok(false);
        }
        Ok(true)
    }
}

pub fn render_retrieval_result(result: &RetrievalResult) -> String {
    let mut lines = vec![format!("[A] anvil > repo-find {}", result.query)];
    if result.matches.is_empty() {
        lines.push("  no matches".to_string());
        return lines.join("\n");
    }

    for item in &result.matches {
        lines.push(format!("  - {} (score {})", item.path, item.score));
        for snippet in &item.snippets {
            lines.push(format!("      {snippet}"));
        }
    }
    lines.join("\n")
}

fn collect_files(root: &Path, files: &mut Vec<IndexedFile>) {
    for path in crate::walk::walk(root) {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        if let Some(file) = metadata_for(&path, &relative) {
            files.push(file);
        }
    }
}

pub fn default_cache_path(state_dir: &Path) -> PathBuf {
    state_dir.join("retrieval-index.json")
}

// ---------------------------------------------------------------------------
// Keyword splitting
// ---------------------------------------------------------------------------

/// Split query into lowercase keyword tokens (space-delimited).
/// Guards: truncate to MAX_QUERY_BYTES, cap at MAX_KEYWORDS.
fn split_keywords(query: &str) -> Vec<String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let capped = if trimmed.len() > MAX_QUERY_BYTES {
        &trimmed[..MAX_QUERY_BYTES]
    } else {
        trimmed
    };
    let mut keywords: Vec<String> = capped
        .split_whitespace()
        .map(|tok| tok.to_ascii_lowercase())
        .collect();
    keywords.truncate(MAX_KEYWORDS);
    keywords
}

// ---------------------------------------------------------------------------
// 1st pass — path/name scoring (no I/O)
// ---------------------------------------------------------------------------

fn score_file_path_only(
    file: &IndexedFile,
    keywords: &[String],
    changed_files: &[String],
    related_tests: &[String],
    neighbor_dirs: &[String],
) -> Option<RetrievalMatch> {
    let path_lc = file.relative_path.to_ascii_lowercase();
    let file_name = Path::new(&file.relative_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let mut score: i32 = 0;
    let mut snippets = Vec::new();

    // Per-keyword path/name scoring (cumulative)
    for kw in keywords {
        if file_name == *kw {
            score += SCORE_FILENAME_EXACT;
            if !snippets
                .iter()
                .any(|s: &String| s.starts_with("file match:"))
            {
                snippets.push(format!("file match: {}", file.relative_path));
            }
        } else if file_name.contains(kw.as_str()) {
            score += SCORE_FILENAME_PARTIAL;
            if !snippets
                .iter()
                .any(|s: &String| s.starts_with("file match:"))
            {
                snippets.push(format!("file match: {}", file.relative_path));
            }
        }

        if path_lc.contains(kw.as_str()) {
            score += SCORE_PATH_PARTIAL;
            if !snippets
                .iter()
                .any(|s: &String| s.starts_with("path match:"))
            {
                snippets.push(format!("path match: {}", file.relative_path));
            }
        }
    }

    // Boost: changed file
    if changed_files.contains(&file.relative_path) {
        score += SCORE_CHANGED_FILE_BOOST;
    }

    // Boost: related test
    if related_tests.contains(&file.relative_path) {
        score += SCORE_RELATED_TEST_BOOST;
    }

    // Boost: neighbor directory
    if let Some(parent) = Path::new(&file.relative_path).parent() {
        let parent_str = parent.to_string_lossy().to_string();
        if neighbor_dirs.contains(&parent_str) {
            score += SCORE_NEIGHBOR_BOOST;
        }
    }

    if score > 0 {
        Some(RetrievalMatch {
            path: file.relative_path.clone(),
            score,
            snippets,
        })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// File I/O (separated from scoring)
// ---------------------------------------------------------------------------

/// Read file content with MAX_CONTENT_SIZE guard. Returns None on failure.
fn read_file_content(root: &Path, relative_path: &str) -> Option<String> {
    let full_path = root.join(relative_path);
    // Ensure the resolved path stays under root
    if !full_path.starts_with(root) {
        return None;
    }
    let should_read = fs::metadata(&full_path)
        .map(|m| m.len() <= MAX_CONTENT_SIZE)
        .unwrap_or(false);
    if !should_read {
        return None;
    }
    fs::read_to_string(&full_path).ok()
}

// ---------------------------------------------------------------------------
// 2nd pass — content + symbol scoring (pure computation on loaded text)
// ---------------------------------------------------------------------------

fn score_content(
    mut candidate: RetrievalMatch,
    content: Option<&str>,
    keywords: &[String],
) -> RetrievalMatch {
    let Some(text) = content else {
        return candidate;
    };

    // Symbol extraction and scoring
    let symbols = extract_symbols(text);
    for kw in keywords {
        for sym in &symbols {
            if sym.to_ascii_lowercase().contains(kw.as_str()) {
                candidate.score += SCORE_SYMBOL_MATCH;
            }
        }
    }

    // Content line scoring (with cap)
    let mut content_line_score: i32 = 0;
    let mut keyword_found_in_content = vec![false; keywords.len()];

    for line in text.lines() {
        let line_lc = line.to_ascii_lowercase();
        for (i, kw) in keywords.iter().enumerate() {
            if line_lc.contains(kw.as_str()) {
                if content_line_score < SCORE_CONTENT_LINE_CAP {
                    content_line_score += SCORE_CONTENT_LINE;
                    // Collect snippets (up to 3 total)
                    if candidate.snippets.len() < 3 {
                        let trimmed = line.trim();
                        let snippet = if trimmed.chars().count() <= 120 {
                            trimmed.to_string()
                        } else {
                            let compact: String = trimmed.chars().take(117).collect();
                            format!("{compact}...")
                        };
                        candidate.snippets.push(snippet);
                    }
                }
                keyword_found_in_content[i] = true;
            }
        }
    }
    // Cap the content line score
    if content_line_score > SCORE_CONTENT_LINE_CAP {
        content_line_score = SCORE_CONTENT_LINE_CAP;
    }
    candidate.score += content_line_score;

    // All-keywords bonus (path + content combined)
    if keywords.len() > 1 && keyword_found_in_content.iter().all(|&found| found) {
        candidate.score += SCORE_ALL_KEYWORDS_BONUS;
    }

    candidate
}

// ---------------------------------------------------------------------------
// Symbol extraction (regex-based, Rust-focused)
// ---------------------------------------------------------------------------

fn extract_symbols(content: &str) -> Vec<String> {
    let re = Regex::new(r"(?:fn|struct|impl|trait|enum|mod|type|const|static)\s+(\w+)")
        .expect("symbol regex should compile");
    re.captures_iter(content)
        .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// Git context helpers
// ---------------------------------------------------------------------------

/// Get changed files via `git diff --name-only HEAD`. Falls back to empty vec.
fn get_changed_files(root: &Path) -> Vec<String> {
    Command::new("git")
        .args(["diff", "--name-only", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| !line.is_empty())
                .map(|line| line.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Find test files related to changed files by stem matching.
fn find_related_tests(files: &[IndexedFile], changed_files: &[String]) -> Vec<String> {
    let stems: Vec<String> = changed_files
        .iter()
        .filter_map(|path| {
            Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .collect();

    if stems.is_empty() {
        return Vec::new();
    }

    files
        .iter()
        .filter(|f| {
            let path_lc = f.relative_path.to_ascii_lowercase();
            if !path_lc.contains("test") {
                return false;
            }
            let file_stem = Path::new(&f.relative_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            stems
                .iter()
                .any(|stem| file_stem.contains(&stem.to_ascii_lowercase()))
        })
        .map(|f| f.relative_path.clone())
        .collect()
}

/// Extract parent directories of changed files.
fn get_neighbor_dirs(changed_files: &[String]) -> Vec<String> {
    let mut dirs: Vec<String> = changed_files
        .iter()
        .filter_map(|path| {
            Path::new(path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
        })
        .filter(|d| !d.is_empty())
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

// ---------------------------------------------------------------------------
// Manifest hashing & metadata
// ---------------------------------------------------------------------------

fn compute_manifest_hash(files: &[IndexedFile]) -> u64 {
    let mut hasher = DefaultHasher::new();
    files.len().hash(&mut hasher);
    for entry in files {
        entry.relative_path.hash(&mut hasher);
        entry.size_bytes.hash(&mut hasher);
        entry.modified_ms.hash(&mut hasher);
    }
    hasher.finish()
}

fn metadata_for(path: &Path, relative_path: &str) -> Option<IndexedFile> {
    let metadata = fs::metadata(path).ok()?;
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_millis())
        .unwrap_or(0);
    Some(IndexedFile {
        relative_path: relative_path.to_string(),
        size_bytes: metadata.len(),
        modified_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_keywords_single_word() {
        assert_eq!(split_keywords("hello"), vec!["hello"]);
    }

    #[test]
    fn split_keywords_multiple_words() {
        assert_eq!(split_keywords("foo bar baz"), vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn split_keywords_empty_returns_empty() {
        assert!(split_keywords("").is_empty());
        assert!(split_keywords("   ").is_empty());
    }

    #[test]
    fn split_keywords_normalizes_to_lowercase() {
        assert_eq!(split_keywords("Hello WORLD"), vec!["hello", "world"]);
    }

    #[test]
    fn split_keywords_caps_at_max_keywords() {
        let query = (0..30)
            .map(|i| format!("kw{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let result = split_keywords(&query);
        assert_eq!(result.len(), MAX_KEYWORDS);
    }

    #[test]
    fn extract_symbols_fn_and_struct() {
        let code = "fn hello() {}\nstruct World;\nimpl World {}\ntrait Foo {}\n";
        let symbols = extract_symbols(code);
        assert!(symbols.contains(&"hello".to_string()));
        assert!(symbols.contains(&"World".to_string()));
        assert!(symbols.contains(&"Foo".to_string()));
    }

    #[test]
    fn extract_symbols_empty_content() {
        assert!(extract_symbols("").is_empty());
        assert!(extract_symbols("let x = 42;").is_empty());
    }

    #[test]
    fn get_neighbor_dirs_extracts_parents() {
        let changed = vec!["src/app/mod.rs".to_string(), "tests/foo.rs".to_string()];
        let dirs = get_neighbor_dirs(&changed);
        assert!(dirs.contains(&"src/app".to_string()));
        assert!(dirs.contains(&"tests".to_string()));
    }

    #[test]
    fn score_content_adds_to_existing_score() {
        let candidate = RetrievalMatch {
            path: "test.rs".to_string(),
            score: 100,
            snippets: vec![],
        };
        let content = "fn hello() {}\nhello world\n";
        let result = score_content(candidate, Some(content), &["hello".to_string()]);
        // Should have: existing 100 + symbol(40) + content lines (2 * 8 = 16)
        assert!(result.score > 100);
    }

    #[test]
    fn content_line_scoring_with_cap() {
        let candidate = RetrievalMatch {
            path: "test.rs".to_string(),
            score: 0,
            snippets: vec![],
        };
        // 20 lines each matching keyword — 20 * 8 = 160, but cap is 80
        let lines: String = (0..20).map(|_| "keyword_match line\n").collect();
        let result = score_content(candidate, Some(&lines), &["keyword_match".to_string()]);
        // content score should be capped at 80
        assert!(result.score <= SCORE_CONTENT_LINE_CAP);
    }

    #[test]
    fn all_keywords_bonus_applied_when_all_match() {
        let candidate = RetrievalMatch {
            path: "test.rs".to_string(),
            score: 0,
            snippets: vec![],
        };
        let content = "alpha beta\n";
        let result = score_content(
            candidate,
            Some(content),
            &["alpha".to_string(), "beta".to_string()],
        );
        // Should include: content lines + all-keywords bonus
        assert!(result.score >= SCORE_ALL_KEYWORDS_BONUS);
    }

    #[test]
    fn all_keywords_bonus_not_applied_for_single_keyword() {
        let candidate = RetrievalMatch {
            path: "test.rs".to_string(),
            score: 0,
            snippets: vec![],
        };
        let content = "alpha line\n";
        let result = score_content(candidate, Some(content), &["alpha".to_string()]);
        // Single keyword: no bonus even when it matches
        assert!(result.score < SCORE_ALL_KEYWORDS_BONUS);
    }

    #[test]
    fn find_related_tests_by_stem() {
        let files = vec![
            IndexedFile {
                relative_path: "src/retrieval.rs".to_string(),
                size_bytes: 100,
                modified_ms: 0,
            },
            IndexedFile {
                relative_path: "tests/retrieval_test.rs".to_string(),
                size_bytes: 100,
                modified_ms: 0,
            },
            IndexedFile {
                relative_path: "tests/unrelated.rs".to_string(),
                size_bytes: 100,
                modified_ms: 0,
            },
        ];
        let changed = vec!["src/retrieval.rs".to_string()];
        let related = find_related_tests(&files, &changed);
        assert!(related.contains(&"tests/retrieval_test.rs".to_string()));
        assert!(!related.contains(&"tests/unrelated.rs".to_string()));
    }
}
