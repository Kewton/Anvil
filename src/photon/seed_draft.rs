//! Issue #592: Photon seed-draft persistence layer.
//!
//! Stores user-explicit feedback (correction / rule) drafts before they are
//! shipped to the photon sidecar via `/v1/evaluate`. Drafts live under
//! `state_root/photon_seed_drafts/<draft_id>.json` with 0o600 permissions and
//! a lazy LRU eviction policy mirroring `case_record.rs` (DR-005 流儀).
//!
//! Security:
//! * `OpenOptions::create_new(true)` + 0o600 perms.
//! * Directory symlink rejected via `symlink_metadata`.
//! * Per-file symlink + non-regular + oversize rejection on listing.
//! * `validate_draft_id` allowlist `[a-z0-9_-]{16,64}` + `draft_` prefix.
//! * Hard size cap `MAX_PHOTON_SEED_DRAFT_BYTES=16384`.
//! * Per-state-root cap `MAX_PHOTON_SEED_DRAFTS=256` with oldest-first eviction.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::session::case_record::RepoFingerprint;

/// Maximum size (in bytes) of a single seed draft JSON file.
pub const MAX_PHOTON_SEED_DRAFT_BYTES: usize = 16 * 1024;

/// Maximum number of seed drafts retained under a single state_root before
/// lazy LRU eviction removes the oldest entries.
pub const MAX_PHOTON_SEED_DRAFTS: usize = 256;

const DRAFT_DIR_NAME: &str = "photon_seed_drafts";
const DRAFT_ID_PREFIX: &str = "draft_";
const DRAFT_ID_BODY_MIN: usize = 16;
const DRAFT_ID_BODY_MAX: usize = 64;

/// On-disk representation of a user-explicit photon seed draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotonSeedDraft {
    pub draft_id: String,
    pub created_at: u64,
    /// Either `"correct"` or `"rule"` — kept as a `String` so the schema is
    /// forward-compatible with future commands without bumping a serde enum.
    pub command: String,
    pub repo_fingerprint: Option<RepoFingerprint>,
    pub source_context_pack_request_id: Option<String>,
    pub originating_summary_id: Option<String>,
    pub user_text: String,
    pub feedback_event_id: String,
}

/// Persistence error type. Mirrors `session::case_record::PersistError`.
#[derive(Debug)]
pub enum SeedDraftPersistError {
    Io(io::Error),
    TooBig { bytes: usize },
    InvalidId(String),
}

impl std::fmt::Display for SeedDraftPersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "seed draft io error: {e}"),
            Self::TooBig { bytes } => write!(
                f,
                "seed draft size {bytes} exceeds MAX_PHOTON_SEED_DRAFT_BYTES ({MAX_PHOTON_SEED_DRAFT_BYTES})"
            ),
            Self::InvalidId(msg) => write!(f, "seed draft invalid id: {msg}"),
        }
    }
}

impl std::error::Error for SeedDraftPersistError {}

impl From<io::Error> for SeedDraftPersistError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for SeedDraftPersistError {
    fn from(e: serde_json::Error) -> Self {
        Self::Io(io::Error::other(e))
    }
}

/// File-system entry produced by `iter_seed_draft_files`. Mirrors
/// `case_record::CaseFileEntry`.
#[derive(Debug, Clone)]
pub struct SeedDraftFileEntry {
    pub draft_id: String,
    pub path: PathBuf,
    pub created_at: u64,
    pub size: u64,
}

/// Derive a deterministic 24-hex-prefixed `draft_<sha-prefix>` id from the
/// draft payload and the creation timestamp. Same `(payload, now_ms)` always
/// yields the same id — used to avoid clashes for fresh drafts within the
/// same millisecond.
#[must_use]
pub fn derive_draft_id(payload: &str, now_ms: i64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload.as_bytes());
    hasher.update(b"\x00");
    hasher.update(now_ms.to_be_bytes());
    let digest = hasher.finalize();
    let hex = format!("{digest:x}");
    // 24 hex chars sit safely inside the 16..=64 allowlist.
    format!("{DRAFT_ID_PREFIX}{}", &hex[..24])
}

/// Validate that `s` matches `draft_[a-z0-9_-]{16,64}` and contains no
/// path-bearing characters. Returns `Ok(())` on success.
pub fn validate_draft_id(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("seed draft id is empty".to_string());
    }
    if !s.starts_with(DRAFT_ID_PREFIX) {
        return Err(format!(
            "seed draft id must start with `{DRAFT_ID_PREFIX}`: {s}"
        ));
    }
    let body = &s[DRAFT_ID_PREFIX.len()..];
    if body.len() < DRAFT_ID_BODY_MIN || body.len() > DRAFT_ID_BODY_MAX {
        return Err(format!(
            "seed draft id body must be {DRAFT_ID_BODY_MIN}..={DRAFT_ID_BODY_MAX} bytes: {s}"
        ));
    }
    if body.starts_with('-') {
        return Err(format!("seed draft id body must not start with '-': {s}"));
    }
    for ch in body.chars() {
        let allowed = ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-';
        if !allowed {
            return Err(format!(
                "seed draft id contains disallowed character '{ch}': {s}"
            ));
        }
    }
    Ok(())
}

/// Persist a `PhotonSeedDraft` under `state_root/photon_seed_drafts/`.
///
/// Performs lazy LRU eviction when the directory holds `>= MAX_PHOTON_SEED_DRAFTS`
/// entries. Rejects directory-level symlinks and per-file symlinks. Returns
/// the number of bytes written.
pub fn persist(state_root: &Path, draft: &PhotonSeedDraft) -> Result<usize, SeedDraftPersistError> {
    validate_draft_id(&draft.draft_id).map_err(SeedDraftPersistError::InvalidId)?;

    let json = serde_json::to_vec_pretty(draft)?;
    if json.len() > MAX_PHOTON_SEED_DRAFT_BYTES {
        return Err(SeedDraftPersistError::TooBig { bytes: json.len() });
    }

    let dir = state_root.join(DRAFT_DIR_NAME);
    // Reject pre-existing directory symlink (defense in depth — state_root is
    // already owner-only on Unix, but the symlink_metadata check costs little).
    if let Ok(meta) = fs::symlink_metadata(&dir)
        && meta.file_type().is_symlink()
    {
        return Err(SeedDraftPersistError::Io(io::Error::other(
            "photon_seed_drafts directory is a symlink (rejected)",
        )));
    }
    fs::create_dir_all(&dir)?;

    // CB-004 (Issue #592): tighten the seed-draft directory to owner-only
    // on Unix. `create_dir_all` honours the umask which may yield 0o755 on
    // typical systems; we explicitly set 0o700 so peer users on a shared
    // host cannot enumerate draft filenames. Skipped on non-unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }

    lazy_evict(state_root)?;

    let path = dir.join(format!("{}.json", draft.draft_id));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        // Best effort — file is already created so we apply chmod after open.
        let _ = fs::set_permissions(&path, perms);
    }

    file.write_all(&json)?;
    Ok(json.len())
}

/// Enumerate seed-draft files under `state_root/photon_seed_drafts/`. Symlinks,
/// non-regular files, oversized files, and invalid stems are skipped. Result
/// is sorted by `created_at` ascending (oldest first) for LRU eviction.
pub fn iter_seed_draft_files(state_root: &Path) -> Vec<SeedDraftFileEntry> {
    let dir = state_root.join(DRAFT_DIR_NAME);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        if meta.len() > MAX_PHOTON_SEED_DRAFT_BYTES as u64 {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if validate_draft_id(stem).is_err() {
            continue;
        }
        let created_at = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push(SeedDraftFileEntry {
            draft_id: stem.to_string(),
            path,
            created_at,
            size: meta.len(),
        });
    }
    out.sort_by_key(|e| e.created_at);
    out
}

/// Lazy LRU eviction: while at or above `MAX_PHOTON_SEED_DRAFTS`, remove the
/// oldest entry. Best-effort; failures to remove are silently swallowed.
///
/// CB-005 (Issue #592): Just before `remove_file`, re-call `symlink_metadata`
/// on the victim path and re-validate (regular file / non-symlink / matching
/// directory / stem allowlist / size cap) so a TOCTOU race that swapped the
/// file for a symlink (or replaced it with a non-draft file) after the scan
/// cannot trick us into unlinking the attacker's chosen path. If validation
/// fails the entry is silently skipped.
fn lazy_evict(state_root: &Path) -> io::Result<()> {
    let expected_dir = state_root.join(DRAFT_DIR_NAME);
    let mut entries = iter_seed_draft_files(state_root);
    while entries.len() >= MAX_PHOTON_SEED_DRAFTS {
        let victim = entries.remove(0);
        if !is_safe_evict_candidate(&victim.path, &expected_dir) {
            continue;
        }
        let _ = std::fs::remove_file(&victim.path);
    }
    Ok(())
}

/// CB-005 re-validation gate. Returns `true` only when the path:
/// * is a regular file (not symlink, not directory, not other);
/// * lives directly inside `expected_dir`;
/// * has a `.json` extension;
/// * has a stem that passes `validate_draft_id`;
/// * is at or under the `MAX_PHOTON_SEED_DRAFT_BYTES` cap.
fn is_safe_evict_candidate(path: &Path, expected_dir: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if parent != expected_dir {
        return false;
    }
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if meta.file_type().is_symlink() {
        return false;
    }
    if !meta.is_file() {
        return false;
    }
    if meta.len() > MAX_PHOTON_SEED_DRAFT_BYTES as u64 {
        return false;
    }
    if path.extension().and_then(|s| s.to_str()) != Some("json") {
        return false;
    }
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    if validate_draft_id(stem).is_err() {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_draft(id: &str) -> PhotonSeedDraft {
        PhotonSeedDraft {
            draft_id: id.to_string(),
            created_at: 0,
            command: "correct".to_string(),
            repo_fingerprint: None,
            source_context_pack_request_id: None,
            originating_summary_id: None,
            user_text: "user said: avoid X".to_string(),
            feedback_event_id: "fb_event_test".to_string(),
        }
    }

    #[test]
    fn derive_draft_id_is_deterministic() {
        let a = derive_draft_id("payload", 1234);
        let b = derive_draft_id("payload", 1234);
        assert_eq!(a, b);
        assert!(validate_draft_id(&a).is_ok());
    }

    #[test]
    fn derive_draft_id_changes_with_inputs() {
        let a = derive_draft_id("payload", 1234);
        let b = derive_draft_id("payload", 1235);
        let c = derive_draft_id("payload2", 1234);
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn validate_draft_id_accepts_normal_ids() {
        assert!(validate_draft_id("draft_abcdef0123456789").is_ok());
        assert!(validate_draft_id("draft_abcdef0123456789abcd").is_ok());
    }

    #[test]
    fn validate_draft_id_rejects_missing_prefix() {
        assert!(validate_draft_id("xxxx_abcdef0123456789abcd").is_err());
    }

    #[test]
    fn validate_draft_id_rejects_too_short() {
        assert!(validate_draft_id("draft_abc").is_err());
    }

    #[test]
    fn validate_draft_id_rejects_too_long() {
        let body: String = "a".repeat(65);
        assert!(validate_draft_id(&format!("draft_{body}")).is_err());
    }

    #[test]
    fn validate_draft_id_rejects_dash_prefix() {
        assert!(validate_draft_id("draft_-deadbeefdeadbeef").is_err());
    }

    #[test]
    fn validate_draft_id_rejects_uppercase() {
        assert!(validate_draft_id("draft_ABCDEF0123456789").is_err());
    }

    #[test]
    fn validate_draft_id_rejects_traversal_and_path_chars() {
        assert!(validate_draft_id("draft_abc/../etc/pass").is_err());
        assert!(validate_draft_id("draft_abc/def0123456789").is_err());
        assert!(validate_draft_id("draft_abc\\def0123456789").is_err());
        assert!(validate_draft_id("draft_abc.def0123456789").is_err());
    }

    #[test]
    fn validate_draft_id_rejects_empty() {
        assert!(validate_draft_id("").is_err());
    }

    #[test]
    fn persist_happy_path_writes_pretty_json() {
        let tmp = tempfile::tempdir().unwrap();
        let id = derive_draft_id("hello", 1);
        let draft = fake_draft(&id);
        let bytes = persist(tmp.path(), &draft).unwrap();
        assert!(bytes > 0);
        let path = tmp.path().join(DRAFT_DIR_NAME).join(format!("{id}.json"));
        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: PhotonSeedDraft = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.draft_id, id);
        assert_eq!(parsed.user_text, "user said: avoid X");
    }

    #[test]
    fn persist_rejects_invalid_id() {
        let tmp = tempfile::tempdir().unwrap();
        let draft = fake_draft("not_a_valid_prefix");
        let err = persist(tmp.path(), &draft).unwrap_err();
        match err {
            SeedDraftPersistError::InvalidId(_) => {}
            other => panic!("expected InvalidId, got {other}"),
        }
    }

    #[test]
    fn persist_rejects_oversize_json() {
        let tmp = tempfile::tempdir().unwrap();
        let id = derive_draft_id("big", 1);
        let mut draft = fake_draft(&id);
        draft.user_text = "x".repeat(MAX_PHOTON_SEED_DRAFT_BYTES + 1);
        let err = persist(tmp.path(), &draft).unwrap_err();
        match err {
            SeedDraftPersistError::TooBig { .. } => {}
            other => panic!("expected TooBig, got {other}"),
        }
    }

    #[test]
    fn persist_evicts_when_at_cap() {
        // Create state_root/photon_seed_drafts populated with MAX entries,
        // verify persist still succeeds (eviction kicks in to make room).
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(DRAFT_DIR_NAME);
        std::fs::create_dir_all(&dir).unwrap();
        // Drop in MAX placeholder files with distinct, valid stems.
        for i in 0..MAX_PHOTON_SEED_DRAFTS {
            let stem = format!("draft_evict{i:0>16}");
            assert!(validate_draft_id(&stem).is_ok(), "stem={stem}");
            let p = dir.join(format!("{stem}.json"));
            std::fs::write(&p, b"{}").unwrap();
        }
        assert_eq!(
            iter_seed_draft_files(tmp.path()).len(),
            MAX_PHOTON_SEED_DRAFTS
        );
        let id = derive_draft_id("new", 1);
        let draft = fake_draft(&id);
        persist(tmp.path(), &draft).unwrap();
        // After persist, total should remain <= MAX (oldest evicted, new added).
        let after = iter_seed_draft_files(tmp.path()).len();
        assert!(
            after <= MAX_PHOTON_SEED_DRAFTS,
            "after eviction the directory should not exceed cap (got {after})"
        );
    }

    #[test]
    fn persist_rejects_directory_symlink() {
        // Build a target dir and symlink state_root/photon_seed_drafts -> target.
        // On platforms without symlink support this test silently passes (the
        // build code returns Ok(())). On Unix this is the real path-confinement
        // hardening assertion.
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let tmp = tempfile::tempdir().unwrap();
            let target = tmp.path().join("target_dir");
            std::fs::create_dir_all(&target).unwrap();
            let link = tmp.path().join(DRAFT_DIR_NAME);
            symlink(&target, &link).unwrap();
            let id = derive_draft_id("sym", 1);
            let draft = fake_draft(&id);
            let err = persist(tmp.path(), &draft).unwrap_err();
            match err {
                SeedDraftPersistError::Io(_) => {}
                other => panic!("expected Io error for symlink dir, got {other}"),
            }
        }
    }

    #[test]
    fn iter_seed_draft_files_skips_invalid_and_oversize() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(DRAFT_DIR_NAME);
        std::fs::create_dir_all(&dir).unwrap();
        // Valid
        std::fs::write(dir.join("draft_abcdef0123456789.json"), b"{}").unwrap();
        // Invalid stem (no prefix)
        std::fs::write(dir.join("nope_abcdef0123456789.json"), b"{}").unwrap();
        // Wrong extension
        std::fs::write(dir.join("draft_abcdef0123456789.txt"), b"{}").unwrap();
        // Oversize
        let huge = vec![b'a'; MAX_PHOTON_SEED_DRAFT_BYTES + 1];
        std::fs::write(dir.join("draft_huge012345678901.json"), &huge).unwrap();
        let listed = iter_seed_draft_files(tmp.path());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].draft_id, "draft_abcdef0123456789");
    }
}
