//! Repository fingerprint for `RepoGraph` cache keying (Issue #468).
//!
//! Combines `git rev-parse HEAD` + `work_root` canonical path + `state_root`
//! canonical path into a stable 16-hex `id()`. On non-Unix platforms (or
//! outside a git repo) `git_rev` is `None`, in which case the id falls back
//! to the path hashes alone (DR2-007). Including `state_root` in the hash
//! avoids cross-test cache collisions (DR1-007).

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::RepoGraphError;
use crate::util::git_hardened::run_git;

/// Internal representation of the cache key. Persisted JSON includes a
/// `schema_version` field so future on-disk format changes can be detected
/// (DR1-008): mismatching versions are treated as corrupt and skipped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Fingerprint {
    pub(crate) git_rev: Option<String>,
    pub(crate) work_root_hash: String,
    pub(crate) state_root_hash: String,
    pub(crate) schema_version: u8,
}

impl Fingerprint {
    /// Stable 16-hex identifier used as the cache filename.
    pub(crate) fn id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.git_rev.as_deref().unwrap_or("").as_bytes());
        hasher.update(b"/");
        hasher.update(self.work_root_hash.as_bytes());
        hasher.update(b"/");
        hasher.update(self.state_root_hash.as_bytes());
        let digest = hasher.finalize();
        let mut hex = String::with_capacity(16);
        for byte in &digest[..8] {
            hex.push_str(&format!("{byte:02x}"));
        }
        hex
    }
}

/// Current on-disk schema. Bump when `RepoGraph` JSON layout changes.
pub(crate) const SCHEMA_VERSION: u8 = 1;

/// Build a fingerprint from current process state. Non-fatal: any failure to
/// canonicalize either path returns `Err(CwdCanonicalFailed)`. Git
/// unavailability is normal on non-Unix and silently produces `git_rev=None`.
pub(crate) fn current_repo_fingerprint(
    work_root: &Path,
    state_root: &Path,
) -> Result<Fingerprint, RepoGraphError> {
    let work_canonical = work_root
        .canonicalize()
        .map_err(|_| RepoGraphError::CwdCanonicalFailed)?;
    // state_root may not exist yet on first run; fall back to the input path
    // rather than failing — the caller will create the directory shortly.
    let state_canonical = state_root
        .canonicalize()
        .unwrap_or_else(|_| state_root.to_path_buf());

    let git_rev = run_git(&work_canonical, &["rev-parse", "HEAD"]);

    Ok(Fingerprint {
        git_rev,
        work_root_hash: sha256_short(&work_canonical.to_string_lossy()),
        state_root_hash: sha256_short(&state_canonical.to_string_lossy()),
        schema_version: SCHEMA_VERSION,
    })
}

pub(crate) fn sha256_short(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let d = h.finalize();
    let mut hex = String::with_capacity(16);
    for byte in &d[..8] {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_returns_16_hex_chars() {
        let fp = Fingerprint {
            git_rev: Some("abc123".into()),
            work_root_hash: "ffff".into(),
            state_root_hash: "0000".into(),
            schema_version: 1,
        };
        let id = fp.id();
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn id_changes_when_state_root_changes() {
        let a = Fingerprint {
            git_rev: Some("rev".into()),
            work_root_hash: "wh".into(),
            state_root_hash: "s1".into(),
            schema_version: 1,
        };
        let b = Fingerprint {
            state_root_hash: "s2".into(),
            ..a.clone()
        };
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn id_stable_when_git_rev_is_none() {
        let fp = Fingerprint {
            git_rev: None,
            work_root_hash: "w".into(),
            state_root_hash: "s".into(),
            schema_version: 1,
        };
        let a = fp.id();
        let b = fp.id();
        assert_eq!(a, b);
    }

    #[test]
    fn current_fingerprint_canonicalizes_existing_paths() {
        let tmp = std::env::temp_dir();
        let work = tmp.join(format!("anvil-fp-test-{}", std::process::id()));
        std::fs::create_dir_all(&work).unwrap();
        let state = work.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let fp = current_repo_fingerprint(&work, &state).unwrap();
        assert_eq!(fp.schema_version, 1);
        assert_eq!(fp.work_root_hash.len(), 16);
        assert_eq!(fp.state_root_hash.len(), 16);
        let _ = std::fs::remove_dir_all(&work);
    }
}
