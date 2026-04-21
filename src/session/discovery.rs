//! Shared iterator that walks `state_root/sessions/*` and yields parsed
//! session snapshots with their on-disk metadata.
//!
//! The defensive checks (UUID v7 directory names only, skip symlinks, skip
//! non-directories, skip missing / oversized / broken `session.json`) are
//! centralized here so that `resolve_session_id`, `sessions list`, and
//! `sessions clean` all share one implementation.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::session::store::SessionSnapshot;

/// Maximum on-disk size of `session.json` that the iterator is willing to
/// read. Larger files are treated as corrupted and skipped; this mirrors the
/// defense that `resolve_session_id` used to apply inline.
pub const MAX_SESSION_JSON_BYTES: u64 = 10 * 1024 * 1024;

/// One `sessions/<uuid>/session.json` entry that survived all defensive
/// checks. `updated_at` is the filesystem mtime of `session.json`; the
/// iterator deliberately does not fall back to directory mtime (which would
/// drift under symlink manipulation).
#[derive(Debug, Clone)]
pub struct SessionDirEntry {
    pub id: String,
    pub dir: PathBuf,
    pub session_json: PathBuf,
    pub updated_at: SystemTime,
    pub snapshot: SessionSnapshot,
}

/// Walk `state_root/sessions/*` and yield one `SessionDirEntry` per
/// well-formed session directory. Entries that fail any defensive check are
/// silently skipped; callers that need visibility into skip reasons should
/// implement their own loop.
///
/// Order is undefined (filesystem order); callers that care about ordering
/// should sort by `updated_at` / `id`.
pub fn iter_session_dirs(state_root: &Path) -> Vec<SessionDirEntry> {
    let sessions_dir = state_root.join("sessions");
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(&sessions_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(dir_name) = entry.file_name().into_string() else {
            continue;
        };
        // Directory name must be a UUID (any version accepted here; the
        // explicit-id path enforces v7 separately via
        // `sessions_cli::validate_session_id_format`).
        if uuid::Uuid::parse_str(&dir_name).is_err() {
            continue;
        }
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }
        let session_json = path.join("session.json");
        if !session_json.exists() {
            continue;
        }
        let Ok(file_meta) = fs::metadata(&session_json) else {
            continue;
        };
        if file_meta.len() > MAX_SESSION_JSON_BYTES {
            continue;
        }
        let updated_at = file_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let Ok(data) = fs::read_to_string(&session_json) else {
            continue;
        };
        let Ok(snapshot) = serde_json::from_str::<SessionSnapshot>(&data) else {
            continue;
        };
        out.push(SessionDirEntry {
            id: dir_name,
            dir: path,
            session_json,
            updated_at,
            snapshot,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_session(dir: &Path, id: &str, workspace_key: &str) -> PathBuf {
        let session_dir = dir.join("sessions").join(id);
        fs::create_dir_all(&session_dir).unwrap();
        let snap = SessionSnapshot {
            id: id.to_string(),
            workspace_key: workspace_key.to_string(),
            ..Default::default()
        };
        let path = session_dir.join("session.json");
        fs::write(&path, serde_json::to_string_pretty(&snap).unwrap()).unwrap();
        path
    }

    #[test]
    fn iter_returns_valid_sessions() {
        let tmp = TempDir::new().unwrap();
        let id1 = uuid::Uuid::now_v7().to_string();
        let id2 = uuid::Uuid::now_v7().to_string();
        write_session(tmp.path(), &id1, "ws-a");
        write_session(tmp.path(), &id2, "ws-b");
        let entries = iter_session_dirs(tmp.path());
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn iter_skips_non_uuid_directory_names() {
        let tmp = TempDir::new().unwrap();
        let bad = tmp.path().join("sessions").join("not-a-uuid");
        fs::create_dir_all(&bad).unwrap();
        File::create(bad.join("session.json"))
            .unwrap()
            .write_all(b"{}")
            .unwrap();
        let entries = iter_session_dirs(tmp.path());
        assert!(entries.is_empty());
    }

    #[test]
    fn iter_skips_broken_json() {
        let tmp = TempDir::new().unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        let session_dir = tmp.path().join("sessions").join(&id);
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join("session.json"), b"{not json").unwrap();
        let entries = iter_session_dirs(tmp.path());
        assert!(entries.is_empty());
    }

    #[test]
    fn iter_skips_oversized_json() {
        let tmp = TempDir::new().unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        let session_dir = tmp.path().join("sessions").join(&id);
        fs::create_dir_all(&session_dir).unwrap();
        // Write > 10 MiB
        let path = session_dir.join("session.json");
        let mut f = File::create(&path).unwrap();
        f.write_all(&vec![b'a'; (MAX_SESSION_JSON_BYTES + 1) as usize])
            .unwrap();
        drop(f);
        let entries = iter_session_dirs(tmp.path());
        assert!(entries.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn iter_skips_symlinks() {
        let tmp = TempDir::new().unwrap();
        let sessions = tmp.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();

        let real_id = uuid::Uuid::now_v7().to_string();
        write_session(tmp.path(), &real_id, "ws-a");

        let link_id = uuid::Uuid::now_v7().to_string();
        std::os::unix::fs::symlink(sessions.join(&real_id), sessions.join(&link_id)).unwrap();

        let entries = iter_session_dirs(tmp.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, real_id);
    }
}
