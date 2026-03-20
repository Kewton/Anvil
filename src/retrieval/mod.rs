use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fmt::{Display, Formatter};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

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
    content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct IndexedFileMeta {
    relative_path: String,
    size_bytes: u64,
    modified_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryIndex {
    #[serde(default)]
    manifest: Vec<IndexedFileMeta>,
    #[serde(default)]
    manifest_hash: u64,
    files: Vec<IndexedFile>,
}

impl RepositoryIndex {
    pub fn build(root: &Path) -> Result<Self, RetrievalError> {
        let mut files = Vec::new();
        let mut manifest = Vec::new();
        collect_files(root, &mut files, &mut manifest);
        manifest.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        let manifest_hash = compute_manifest_hash(&manifest);
        Ok(Self {
            manifest,
            manifest_hash,
            files,
        })
    }

    pub fn load_or_build(root: &Path, cache_path: &Path) -> Result<Self, RetrievalError> {
        if cache_path.exists() {
            let bytes = fs::read(cache_path).map_err(RetrievalError::CacheRead)?;
            let index: RepositoryIndex =
                serde_json::from_slice(&bytes).map_err(RetrievalError::CacheDecode)?;
            if index.is_current_for(root)? {
                return Ok(index);
            }
        }

        let index = Self::build(root)?;
        index.save(cache_path)?;
        Ok(index)
    }

    pub fn search(&self, query: &str, limit: usize) -> RetrievalResult {
        let needle = query.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return RetrievalResult {
                query: query.to_string(),
                matches: Vec::new(),
            };
        }

        let mut matches = self
            .files
            .iter()
            .filter_map(|file| score_file(file, &needle))
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.path.cmp(&right.path))
        });
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
        collect_manifest(root, &mut current);
        current.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        let current_hash = compute_manifest_hash(&current);
        if current_hash != self.manifest_hash {
            return Ok(false);
        }
        // Hash matched — skip full comparison for speed
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

fn collect_files(root: &Path, files: &mut Vec<IndexedFile>, manifest: &mut Vec<IndexedFileMeta>) {
    for path in crate::walk::walk(root) {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        if let Some(meta) = metadata_for(&path, &relative) {
            manifest.push(meta);
        }

        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        files.push(IndexedFile {
            relative_path: relative,
            content,
        });
    }
}

fn collect_manifest(root: &Path, manifest: &mut Vec<IndexedFileMeta>) {
    for path in crate::walk::walk(root) {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        if let Some(meta) = metadata_for(&path, &relative) {
            manifest.push(meta);
        }
    }
}

pub fn default_cache_path(state_dir: &Path) -> PathBuf {
    state_dir.join("retrieval-index.json")
}

fn score_file(file: &IndexedFile, needle: &str) -> Option<RetrievalMatch> {
    let path_lc = file.relative_path.to_ascii_lowercase();
    let file_name = Path::new(&file.relative_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut score = 0;
    let mut snippets = Vec::new();

    if file_name == needle {
        score += 140;
        snippets.push(format!("file match: {}", file.relative_path));
    } else if file_name.contains(needle) {
        score += 100;
        snippets.push(format!("file match: {}", file.relative_path));
    }

    if path_lc.contains(needle) {
        score += 60;
        if !snippets.iter().any(|item| item.starts_with("path match:")) {
            snippets.push(format!("path match: {}", file.relative_path));
        }
    }

    for line in file.content.lines() {
        if line.to_ascii_lowercase().contains(needle) {
            score += 12;
            if snippets.len() < 3 {
                snippets.push(compact_line(line));
            }
        }
    }

    (score > 0).then(|| RetrievalMatch {
        path: file.relative_path.clone(),
        score,
        snippets,
    })
}

fn compact_line(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.chars().count() <= 120 {
        trimmed.to_string()
    } else {
        let compact: String = trimmed.chars().take(117).collect();
        format!("{compact}...")
    }
}

fn compute_manifest_hash(manifest: &[IndexedFileMeta]) -> u64 {
    let mut hasher = DefaultHasher::new();
    manifest.len().hash(&mut hasher);
    let mut total_size: u64 = 0;
    let mut max_mtime: u128 = 0;
    for entry in manifest {
        total_size += entry.size_bytes;
        if entry.modified_ms > max_mtime {
            max_mtime = entry.modified_ms;
        }
    }
    total_size.hash(&mut hasher);
    max_mtime.hash(&mut hasher);
    hasher.finish()
}

fn metadata_for(path: &Path, relative_path: &str) -> Option<IndexedFileMeta> {
    let metadata = fs::metadata(path).ok()?;
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_millis())
        .unwrap_or(0);
    Some(IndexedFileMeta {
        relative_path: relative_path.to_string(),
        size_bytes: metadata.len(),
        modified_ms,
    })
}
