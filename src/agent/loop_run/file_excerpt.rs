//! File excerpt + content-hash helpers extracted from `turn.rs` (parent
//! #680).
//!
//! Hosts the safe excerpt-reader trio (`open_excerpt_file_nofollow`,
//! `utf8_prefix_respecting_cap`, `truncate_on_char_boundary`) used to render
//! bounded file previews into log payloads and the workspace content-hash
//! SSOT (`current_file_hash_for_relative_path`, `sha256_hex`).
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::safety::path_guard::resolve_user_path;

pub(super) fn truncate_on_char_boundary(s: String, cap: usize) -> String {
    if s.len() <= cap {
        return s;
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Issue #636 (CB-001): return the longest valid UTF-8 prefix of `buf`
/// up to `cap` bytes. If the bytes inside `[0, cap)` are a valid UTF-8
/// prefix but the cap+1 read tail straddles a multi-byte char, we shrink
/// to `valid_up_to()` instead of rejecting the whole excerpt. Truly
/// invalid UTF-8 (`error_len().is_some()`) still returns `None`.
pub(super) fn utf8_prefix_respecting_cap(buf: &[u8], cap: usize) -> Option<&str> {
    let end = buf.len().min(cap);
    let slice = &buf[..end];
    match std::str::from_utf8(slice) {
        Ok(s) => Some(s),
        Err(e) => {
            if e.error_len().is_some() {
                return None;
            }
            // Tail of `slice` is a partial multi-byte char — safe to clip.
            let valid = e.valid_up_to();
            std::str::from_utf8(&slice[..valid]).ok()
        }
    }
}

/// Issue #636 (CB-002): open `target` for read with `O_NOFOLLOW` on Unix
/// so a symlink swap between the workspace-confinement check and the
/// open call cannot redirect us outside the workspace. On non-Unix we
/// fall back to plain `File::open` and rely on the pre-open path checks.
pub(super) fn open_excerpt_file_nofollow(target: &Path) -> Option<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(target)
            .ok()?;
        // Re-check post-open: O_NOFOLLOW guards the final component, and
        // `metadata()` (vs `symlink_metadata`) reflects the opened inode.
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        Some(file)
    }
    #[cfg(not(unix))]
    {
        std::fs::File::open(target).ok()
    }
}

pub(super) fn current_file_hash_for_relative_path(
    work_root: &Path,
    relative_path: &str,
) -> Option<String> {
    let target = resolve_user_path(work_root, relative_path).ok()?;
    let root = std::fs::canonicalize(work_root).unwrap_or_else(|_| work_root.to_path_buf());
    if target.strip_prefix(root).is_err() || !target.is_file() {
        return None;
    }
    std::fs::read(target)
        .ok()
        .map(|bytes| sha256_hex(bytes.as_slice()))
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
