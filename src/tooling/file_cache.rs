//! File read cache for reducing redundant `file.read` operations.
//!
//! Caches file content keyed by canonical path, validated against mtime.
//! LRU eviction with dual limits (entry count + byte size).
//! All public methods perform canonicalize + sandbox boundary check internally.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

/// Default maximum number of cached entries.
const DEFAULT_MAX_ENTRIES: usize = 100;

/// Default maximum total bytes across all cached entries (10 MB).
const DEFAULT_MAX_BYTES: usize = 10_485_760;

/// A single cached file entry.
pub struct CacheEntry {
    pub content: String,
    pub mtime: SystemTime,
    pub byte_size: usize,
    pub last_access: Instant,
}

/// In-memory file read cache with LRU eviction and sandbox boundary enforcement.
///
/// Design notes:
/// - LRU tracking uses `CacheEntry.last_access` only (Single Source of Truth, DR1-001).
/// - Eviction does linear scan on `last_access`; max 100 entries so performance is fine.
/// - Eviction pattern is similar to `CheckpointStack::evict_oldest_while` but uses
///   LRU (last_access oldest) instead of FIFO (DR1-002).
pub struct FileReadCache {
    root: PathBuf,
    entries: HashMap<PathBuf, CacheEntry>,
    total_bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

impl FileReadCache {
    /// Create a new cache with default limits.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            entries: HashMap::new(),
            total_bytes: 0,
            max_entries: DEFAULT_MAX_ENTRIES,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    /// Create a new cache with custom limits.
    pub fn with_limits(root: PathBuf, max_entries: usize, max_bytes: usize) -> Self {
        Self {
            root,
            entries: HashMap::new(),
            total_bytes: 0,
            max_entries,
            max_bytes,
        }
    }

    /// High-level API: check cache for a file, returning content if hit.
    ///
    /// Internally performs canonicalize + mtime check + sandbox boundary validation.
    /// Returns `None` on miss, canonicalize failure, or sandbox violation.
    /// Updates `last_access` on hit (DR1-005).
    pub fn try_get(&mut self, resolved_path: &Path) -> Option<String> {
        let canonical = self.validate_canonical_path(resolved_path)?;
        let mtime = fs::metadata(&canonical).ok()?.modified().ok()?;

        let entry = self.entries.get_mut(&canonical)?;
        if entry.mtime != mtime {
            // mtime changed — stale entry, remove it
            let byte_size = entry.byte_size;
            self.entries.remove(&canonical);
            self.total_bytes = self.total_bytes.saturating_sub(byte_size);
            return None;
        }

        entry.last_access = Instant::now();
        Some(entry.content.clone())
    }

    /// High-level API: record a file's content in the cache.
    ///
    /// Internally performs canonicalize + sandbox validation + LRU eviction.
    /// No-op if canonicalize fails or path is outside sandbox.
    pub fn record(&mut self, resolved_path: &Path, content: String) {
        let canonical = match self.validate_canonical_path(resolved_path) {
            Some(p) => p,
            None => return,
        };
        let mtime = match fs::metadata(&canonical).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => return,
        };

        let byte_size = content.len();

        // Remove existing entry if present (update case)
        if let Some(old) = self.entries.remove(&canonical) {
            self.total_bytes = self.total_bytes.saturating_sub(old.byte_size);
        }

        // Insert new entry
        self.entries.insert(
            canonical,
            CacheEntry {
                content,
                mtime,
                byte_size,
                last_access: Instant::now(),
            },
        );
        self.total_bytes += byte_size;

        // LRU eviction: while over limits, remove oldest entry
        self.evict_while_over_limits();
    }

    /// Invalidate (remove) a cached entry for the given path.
    ///
    /// Internally performs canonicalize. No-op if canonicalize fails or entry not found.
    pub fn invalidate(&mut self, resolved_path: &Path) {
        if let Some(canonical) = self.validate_canonical_path(resolved_path)
            && let Some(old) = self.entries.remove(&canonical)
        {
            self.total_bytes = self.total_bytes.saturating_sub(old.byte_size);
        }
    }

    /// Clear all cached entries (e.g., on session switch).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
    }

    /// Resolve a path to its canonical form and verify it is within the sandbox root.
    ///
    /// Returns `None` if canonicalize fails or the path is outside root.
    /// Security: all public API methods go through this to prevent cache poisoning
    /// and sandbox boundary escape.
    pub fn validate_canonical_path(&self, resolved_path: &Path) -> Option<PathBuf> {
        let canonical = fs::canonicalize(resolved_path).ok()?;
        let root_canonical = fs::canonicalize(&self.root).ok()?;
        if canonical.starts_with(&root_canonical) {
            Some(canonical)
        } else {
            None
        }
    }

    /// Return the number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Return the total cached bytes.
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Evict LRU entries while over capacity limits.
    fn evict_while_over_limits(&mut self) {
        while self.entries.len() > self.max_entries || self.total_bytes > self.max_bytes {
            // Find the entry with the oldest last_access
            let oldest_key = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(key, _)| key.clone());

            match oldest_key {
                Some(key) => {
                    if let Some(removed) = self.entries.remove(&key) {
                        self.total_bytes = self.total_bytes.saturating_sub(removed.byte_size);
                    }
                }
                None => break, // should not happen, but safety
            }
        }
    }
}
