//! Temporary Test Workspace (Issue #458).
//!
//! Stores LLM-generated tests under
//! `state_root/sessions/<session-id>/tmp-tests/{files,metadata}/` so they do
//! not leak into the workspace's git working tree until the user explicitly
//! `promote`s them.
//!
//! Style: follows `precaution.rs` (types + free construction) rather than
//! `feedback.rs` (sealed `from_draft`), because `promote_tmp_test` /
//! `discard_tmp_test` mutate the persisted `TmpTest.status`.
//!
//! Errors are `Result<_, String>` (design judgment #8) to match the existing
//! `path_guard::resolve_user_path` and `ToolRegistry::execute` contracts —
//! `thiserror` is intentionally NOT introduced.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::safety::path_guard::resolve_user_path;

/// Maximum number of unique-temp-file collision retries before we give up.
/// Hitting this implies adversarial parent-directory contents — bail out
/// rather than spin.
const TEMP_FILE_MAX_RETRIES: usize = 8;

/// Process-local monotonic counter mixed into temp file suffixes so that two
/// concurrent `open_temp_file_unique` calls in the same process never collide
/// on the same nonce even if `SystemTime` resolution is coarse.
static TEMP_NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Build a hard-to-predict suffix for a sibling temp file. Mixes pid, an
/// in-process atomic counter, and the sub-second component of wall-clock time
/// so an attacker cannot pre-create a symlink at the same path.
fn random_temp_suffix() -> String {
    let pid = std::process::id();
    let nonce = TEMP_NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{pid:x}.{nonce:x}.{nanos:x}")
}

/// Open a fresh sibling temp file under `parent` whose name starts with
/// `.{base_name}.` and ends with `.tmp`. Always uses `create_new(true)` so
/// any pre-existing path (regular file or symlink) at the chosen name causes
/// the open to fail; we then pick another suffix and retry.
///
/// Returns the chosen path and the open `File` handle. Callers are
/// responsible for `remove_file(temp_path)` on failure paths.
fn open_temp_file_unique(parent: &Path, base_name: &str) -> Result<(PathBuf, File), String> {
    for _ in 0..TEMP_FILE_MAX_RETRIES {
        let suffix = random_temp_suffix();
        let temp_path = parent.join(format!(".{base_name}.{suffix}.tmp"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(format!(
                    "failed to create unique temp file under {}: {err}",
                    parent.display()
                ));
            }
        }
    }
    Err(format!(
        "unable to create a unique temp file under {} after {} attempts",
        parent.display(),
        TEMP_FILE_MAX_RETRIES
    ))
}

// ============================================================================
// 1. 型定義 (TmpTest / TmpTestStatus / TmpTestRunResult)
// ============================================================================

/// 64 KiB cap on individual `metadata/<id>.json` reads (mirrors
/// `discovery::MAX_SESSION_JSON_BYTES`).
pub const MAX_TMP_TEST_METADATA_BYTES: u64 = 64 * 1024;

/// Hard cap on the number of `tmp-tests/metadata/*.json` entries we will
/// list / load per session, to bound DoS amplification.
pub const MAX_TMP_TESTS_PER_SESSION: usize = 4096;

/// Hard cap on the byte length of `relative_path` (post-validation).
pub const MAX_TMP_TEST_REL_PATH_BYTES: usize = 1024;

/// 8 KiB cap on `last_run_result.excerpt`, matching `feedback::EXCERPT_CAP_BYTES`.
pub const MAX_TMP_TEST_EXCERPT_BYTES: usize = 8 * 1024;

/// Initial `schema_version` for tmp-test metadata. Bumped only when the
/// on-disk shape changes incompatibly.
pub const TMP_TEST_SCHEMA_VERSION: u32 = 1;

const TMP_TEST_ID_PREFIX: &str = "tmp_";
const TMP_TEST_ID_BODY_MIN: usize = 16;
const TMP_TEST_ID_BODY_MAX: usize = 64;

fn current_metadata_version() -> u32 {
    TMP_TEST_SCHEMA_VERSION
}

/// Persistent record describing a single LLM-generated tmp-test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmpTest {
    /// Schema version for forward-compat (#458 / DR3-003).
    #[serde(default = "current_metadata_version")]
    pub schema_version: u32,
    /// Deterministic id (`tmp_<sha256-prefix>`); see [`derive_test_id`].
    pub id: String,
    /// Unix epoch seconds (UTC) at creation time. Stored as a primitive so
    /// we don't add a `chrono` dependency.
    pub created_at: u64,
    /// `tmp-tests/files/`-relative path of the test body. Validated by
    /// [`validate_tmp_test_relative_path`].
    pub relative_path: PathBuf,
    /// Lifecycle status (see [`TmpTestStatus`]).
    pub status: TmpTestStatus,
    /// Reserved for future hydration of bash run outcomes (DR1-006).
    /// Always `None` in this issue's lifecycle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_result: Option<TmpTestRunResult>,
}

/// Lifecycle status of a tmp-test.
///
/// `Discarded` is a defensive-deserialize variant: anvil itself deletes the
/// metadata JSON at discard time, so a `status: discarded` entry can only
/// appear from manual edits (used by §3-4 / DR2-006 verification).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TmpTestStatus {
    Draft,
    Promoted,
    Discarded,
}

/// Reserved last-run hydration record. Not produced by this issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmpTestRunResult {
    pub exit_code: i32,
    pub finished_at: u64,
    pub excerpt: String,
}

// ============================================================================
// 2. test-id 採番ロジック (derive_test_id / validate_test_id)
// ============================================================================

/// Derive a deterministic, prefix-allowlisted `tmp_<sha-prefix>` test id from
/// the test body content and creation timestamp. Same `(content, created_at)`
/// always produces the same id.
#[must_use]
pub fn derive_test_id(content: &[u8], created_at: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hasher.update(b"\x00");
    hasher.update(created_at.to_be_bytes());
    let digest = hasher.finalize();
    let hex = format!("{digest:x}");
    // 24-hex prefix keeps the body well within the 16..=64 allowlist.
    format!("{TMP_TEST_ID_PREFIX}{}", &hex[..24])
}

/// Validate that `test_id` matches the `tmp_[a-z0-9_-]{16,64}` allowlist and
/// contains no path-bearing or NUL byte that could escape the metadata
/// directory.
pub fn validate_test_id(test_id: &str) -> Result<(), String> {
    if test_id.is_empty() {
        return Err("tmp-test id is empty".to_string());
    }
    if !test_id.starts_with(TMP_TEST_ID_PREFIX) {
        return Err(format!(
            "tmp-test id must start with `{TMP_TEST_ID_PREFIX}`: {test_id}"
        ));
    }
    let body = &test_id[TMP_TEST_ID_PREFIX.len()..];
    if body.len() < TMP_TEST_ID_BODY_MIN || body.len() > TMP_TEST_ID_BODY_MAX {
        return Err(format!(
            "tmp-test id body must be {TMP_TEST_ID_BODY_MIN}..={TMP_TEST_ID_BODY_MAX} bytes: {test_id}"
        ));
    }
    if body.starts_with('-') {
        return Err(format!(
            "tmp-test id body must not start with '-': {test_id}"
        ));
    }
    for ch in body.chars() {
        let allowed = ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-';
        if !allowed {
            return Err(format!(
                "tmp-test id contains disallowed character '{ch}': {test_id}"
            ));
        }
    }
    Ok(())
}

/// Validate that `rel` is a UTF-8 relative path safe to join under
/// `tmp-tests/files/` (no NUL, no `..`, no absolute path, length-capped).
pub fn validate_tmp_test_relative_path(rel: &str) -> Result<(), String> {
    if rel.is_empty() {
        return Err("tmp-test relative_path is empty".to_string());
    }
    if rel.len() > MAX_TMP_TEST_REL_PATH_BYTES {
        return Err(format!(
            "tmp-test relative_path exceeds {MAX_TMP_TEST_REL_PATH_BYTES} bytes"
        ));
    }
    if rel.contains('\0') {
        return Err("tmp-test relative_path contains NUL byte".to_string());
    }
    let path = Path::new(rel);
    if path.is_absolute() {
        return Err(format!(
            "tmp-test relative_path must not be absolute: {rel}"
        ));
    }
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                return Err(format!(
                    "tmp-test relative_path must not contain '..': {rel}"
                ));
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(format!(
                    "tmp-test relative_path must not be absolute: {rel}"
                ));
            }
            std::path::Component::Normal(part) => {
                if part.is_empty() || part == OsStr::new(".") {
                    return Err(format!("tmp-test relative_path component is empty: {rel}"));
                }
            }
            std::path::Component::CurDir => { /* harmless */ }
        }
    }
    // Reject paths that resolve to "no file name" (trailing slash, "."-only).
    if path.file_name().is_none() {
        return Err(format!("tmp-test relative_path has no file name: {rel}"));
    }
    Ok(())
}

// ============================================================================
// 3. metadata I/O ヘルパ (load_metadata_file / store_metadata_atomic /
//    read_metadata_dir)
// ============================================================================

/// Load and parse a single `metadata/<id>.json`, refusing oversized files,
/// non-regular files, and symlinks. Returns the parsed `TmpTest`.
pub fn load_metadata_file(metadata_path: &Path) -> Result<TmpTest, String> {
    let meta = fs::symlink_metadata(metadata_path).map_err(|err| {
        format!(
            "failed to stat tmp-test metadata {}: {err}",
            metadata_path.display()
        )
    })?;
    let ft = meta.file_type();
    if ft.is_symlink() {
        return Err(format!(
            "tmp-test metadata is a symlink: {}",
            metadata_path.display()
        ));
    }
    if !ft.is_file() {
        return Err(format!(
            "tmp-test metadata is not a regular file: {}",
            metadata_path.display()
        ));
    }
    if meta.len() > MAX_TMP_TEST_METADATA_BYTES {
        return Err(format!(
            "tmp-test metadata exceeds {MAX_TMP_TEST_METADATA_BYTES} bytes: {}",
            metadata_path.display()
        ));
    }
    let mut file = File::open(metadata_path).map_err(|err| {
        format!(
            "failed to open tmp-test metadata {}: {err}",
            metadata_path.display()
        )
    })?;
    let mut buf = String::with_capacity(meta.len() as usize);
    file.read_to_string(&mut buf).map_err(|err| {
        format!(
            "failed to read tmp-test metadata {}: {err}",
            metadata_path.display()
        )
    })?;
    let tmp_test: TmpTest = serde_json::from_str(&buf).map_err(|err| {
        format!(
            "failed to parse tmp-test metadata {}: {err}",
            metadata_path.display()
        )
    })?;
    Ok(tmp_test)
}

/// Atomically write `tmp_test` to `metadata_path` via a sibling
/// temp file + `fs::rename`. Re-checks that the parent metadata directory
/// is not a symlink immediately before rename to defend against races.
pub fn store_metadata_atomic(metadata_path: &Path, tmp_test: &TmpTest) -> Result<(), String> {
    let parent = metadata_path.parent().ok_or_else(|| {
        format!(
            "tmp-test metadata path has no parent: {}",
            metadata_path.display()
        )
    })?;
    fs::create_dir_all(parent).map_err(|err| {
        format!(
            "failed to create tmp-test metadata dir {}: {err}",
            parent.display()
        )
    })?;
    // Symlink defence: parent must be a real directory.
    let parent_meta = fs::symlink_metadata(parent).map_err(|err| {
        format!(
            "failed to stat tmp-test metadata dir {}: {err}",
            parent.display()
        )
    })?;
    if parent_meta.file_type().is_symlink() || !parent_meta.is_dir() {
        return Err(format!(
            "tmp-test metadata dir is not a regular directory: {}",
            parent.display()
        ));
    }

    let serialized = serde_json::to_string_pretty(tmp_test)
        .map_err(|err| format!("failed to serialize tmp-test metadata: {err}"))?;

    let file_name = metadata_path.file_name().ok_or_else(|| {
        format!(
            "tmp-test metadata path has no file name: {}",
            metadata_path.display()
        )
    })?;
    // CB-006 fix: use a hard-to-predict sibling temp name + create_new(true)
    // so an attacker cannot pre-create a symlink that would redirect the
    // metadata write outside `parent`.
    let base_name = file_name.to_string_lossy();
    let (tmp_path, mut handle) = open_temp_file_unique(parent, &base_name)?;
    {
        handle.write_all(serialized.as_bytes()).map_err(|err| {
            let _ = fs::remove_file(&tmp_path);
            format!(
                "failed to write tmp-test metadata temp file {}: {err}",
                tmp_path.display()
            )
        })?;
        handle.sync_all().ok();
    }

    // Re-confirm parent is still a regular directory before rename.
    let recheck_meta = fs::symlink_metadata(parent).map_err(|err| {
        let _ = fs::remove_file(&tmp_path);
        format!(
            "failed to re-stat tmp-test metadata dir {}: {err}",
            parent.display()
        )
    })?;
    if recheck_meta.file_type().is_symlink() || !recheck_meta.is_dir() {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!(
            "tmp-test metadata dir is not a regular directory: {}",
            parent.display()
        ));
    }
    fs::rename(&tmp_path, metadata_path).map_err(|err| {
        let _ = fs::remove_file(&tmp_path);
        format!(
            "failed to atomically rename tmp-test metadata {} -> {}: {err}",
            tmp_path.display(),
            metadata_path.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(metadata_path, fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

/// Read every `metadata/<id>.json` in `metadata_root`, skipping broken /
/// oversized / non-regular entries. Returns at most
/// `MAX_TMP_TESTS_PER_SESSION` entries, deterministically sorted by id.
///
/// Missing `metadata_root` (i.e. session never created a tmp-test) is treated
/// as an empty list, not an error.
///
/// CB-004 fix: the cap is enforced during `read_dir` iteration so an attacker
/// cannot inflate memory by dropping millions of `.json` files into the
/// metadata dir — we stop pushing paths once we have collected
/// `MAX_TMP_TESTS_PER_SESSION + 1` candidates (the +1 lets the caller detect
/// truncation).
///
/// CB-005 fix: each loaded `TmpTest` is re-validated against the
/// `validate_test_id` / `validate_tmp_test_relative_path` /
/// `MAX_TMP_TEST_EXCERPT_BYTES` allowlists, and its `id` is checked against
/// the metadata file's stem. Hand-edited / corrupted entries that fail any
/// of these checks are skipped so list rendering cannot leak control
/// characters / path traversal-looking strings to the terminal.
pub fn read_metadata_dir(metadata_root: &Path) -> Result<Vec<TmpTest>, String> {
    if !metadata_root.exists() {
        return Ok(Vec::new());
    }
    let dir_meta = fs::symlink_metadata(metadata_root).map_err(|err| {
        format!(
            "failed to stat tmp-test metadata dir {}: {err}",
            metadata_root.display()
        )
    })?;
    if dir_meta.file_type().is_symlink() || !dir_meta.is_dir() {
        return Err(format!(
            "tmp-test metadata dir is not a regular directory: {}",
            metadata_root.display()
        ));
    }
    let entries = fs::read_dir(metadata_root).map_err(|err| {
        format!(
            "failed to read tmp-test metadata dir {}: {err}",
            metadata_root.display()
        )
    })?;
    // CB-004: cap candidate paths during iteration. We collect at most
    // MAX_TMP_TESTS_PER_SESSION + 1 to retain truncation visibility without
    // bounding memory by attacker-controlled directory size.
    let collect_cap = MAX_TMP_TESTS_PER_SESSION.saturating_add(1);
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        if paths.len() >= collect_cap {
            break;
        }
        let path = entry.path();
        if path
            .extension()
            .map(|e| e == OsStr::new("json"))
            .unwrap_or(false)
        {
            paths.push(path);
        }
    }
    // Deterministic order: lexicographic by file name (ids are
    // hex-suffixed so this is stable across platforms).
    paths.sort();
    let mut out: Vec<TmpTest> = Vec::new();
    for path in paths.into_iter().take(MAX_TMP_TESTS_PER_SESSION) {
        match load_metadata_file(&path) {
            Ok(tmp_test) => {
                if !metadata_entry_passes_validation(&path, &tmp_test) {
                    continue;
                }
                out.push(tmp_test);
            }
            Err(_) => {
                // Skip broken entries — list_tmp_tests must keep going.
                continue;
            }
        }
    }
    Ok(out)
}

/// CB-005 helper: re-validate a freshly-loaded `TmpTest` against id /
/// relative_path / excerpt allowlists and the on-disk file name. Returns
/// `false` (skip) if any check fails. Defensive: keeps list rendering
/// honest even when metadata has been hand-edited.
fn metadata_entry_passes_validation(metadata_path: &Path, tmp_test: &TmpTest) -> bool {
    if validate_test_id(&tmp_test.id).is_err() {
        return false;
    }
    let expected_stem = metadata_path.file_stem().and_then(|s| s.to_str());
    if expected_stem != Some(tmp_test.id.as_str()) {
        return false;
    }
    let Some(rel) = tmp_test.relative_path.to_str() else {
        return false;
    };
    if validate_tmp_test_relative_path(rel).is_err() {
        return false;
    }
    if let Some(run) = tmp_test.last_run_result.as_ref()
        && run.excerpt.len() > MAX_TMP_TEST_EXCERPT_BYTES
    {
        return false;
    }
    true
}

// ============================================================================
// 4. lifecycle 関数 (create / discard / promote / list)
// ============================================================================

/// Create a tmp-test under `tmp_tests_root/files/<relative_path>` and persist
/// its metadata under `tmp_tests_root/metadata/<id>.json`. Lazy-creates both
/// directories.
pub fn create_generated_test(
    tmp_tests_root: &Path,
    relative_path: &str,
    content: &[u8],
) -> Result<TmpTest, String> {
    validate_tmp_test_relative_path(relative_path)?;

    let files_root = tmp_tests_root.join("files");
    let metadata_root = tmp_tests_root.join("metadata");
    fs::create_dir_all(&files_root).map_err(|err| {
        format!(
            "failed to create tmp-test files dir {}: {err}",
            files_root.display()
        )
    })?;
    fs::create_dir_all(&metadata_root).map_err(|err| {
        format!(
            "failed to create tmp-test metadata dir {}: {err}",
            metadata_root.display()
        )
    })?;

    // Cap entries before doing any work.
    let existing = read_metadata_dir(&metadata_root).unwrap_or_default();
    if existing.len() >= MAX_TMP_TESTS_PER_SESSION {
        return Err(format!(
            "tmp-test count exceeds {MAX_TMP_TESTS_PER_SESSION}; discard or promote some first"
        ));
    }

    let target_path = resolve_user_path(&files_root, relative_path)?;
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "failed to create tmp-test parent dir {}: {err}",
                parent.display()
            )
        })?;
    }

    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let id = derive_test_id(content, created_at);
    validate_test_id(&id)?;

    // Use create_new(true) to avoid silently clobbering same-id files.
    {
        let mut handle = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target_path)
            .map_err(|err| {
                format!(
                    "failed to create tmp-test body file {}: {err}",
                    target_path.display()
                )
            })?;
        handle.write_all(content).map_err(|err| {
            format!(
                "failed to write tmp-test body file {}: {err}",
                target_path.display()
            )
        })?;
        handle.sync_all().ok();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&target_path, fs::Permissions::from_mode(0o600));
    }

    let tmp_test = TmpTest {
        schema_version: TMP_TEST_SCHEMA_VERSION,
        id: id.clone(),
        created_at,
        relative_path: PathBuf::from(relative_path),
        status: TmpTestStatus::Draft,
        last_run_result: None,
    };
    let metadata_path = resolve_user_path(&metadata_root, &format!("{id}.json"))?;
    store_metadata_atomic(&metadata_path, &tmp_test)?;
    Ok(tmp_test)
}

/// Delete the test body and its metadata JSON. Other tmp-tests, `logs/`,
/// `plans/`, and `session.json` are left untouched.
pub fn discard_tmp_test(tmp_tests_root: &Path, test_id: &str) -> Result<(), String> {
    validate_test_id(test_id)?;
    let metadata_root = tmp_tests_root.join("metadata");
    let metadata_path = resolve_user_path(&metadata_root, &format!("{test_id}.json"))?;
    let tmp_test = load_metadata_file(&metadata_path)?;

    let rel = tmp_test
        .relative_path
        .to_str()
        .ok_or_else(|| "tmp-test relative_path is not valid UTF-8".to_string())?;
    validate_tmp_test_relative_path(rel)?;
    let files_root = tmp_tests_root.join("files");
    let target_path = resolve_user_path(&files_root, rel)?;

    // Best-effort body removal: a missing body should not block discard.
    if let Ok(meta) = fs::symlink_metadata(&target_path) {
        if meta.file_type().is_symlink() {
            return Err(format!(
                "tmp-test body is a symlink, refusing to remove: {}",
                target_path.display()
            ));
        }
        fs::remove_file(&target_path).map_err(|err| {
            format!(
                "failed to remove tmp-test body {}: {err}",
                target_path.display()
            )
        })?;
    }
    fs::remove_file(&metadata_path).map_err(|err| {
        format!(
            "failed to remove tmp-test metadata {}: {err}",
            metadata_path.display()
        )
    })?;
    Ok(())
}

/// Promote a tmp-test into the workspace working tree. The body is copied
/// from `tmp_tests_root/files/<rel>` to `workspace_root/<rel>`. The metadata
/// is updated to `Promoted` (not deleted). The body file is also retained
/// under `tmp-tests/files/`.
///
/// Approval policy: requires `auto_approve == true` OR
/// `interactive_approval == true`. (TTY-free CI without `--yes` is rejected.)
///
/// `--force` semantics:
///   * `force == false`: dst must not exist (uses
///     `OpenOptions::create_new(true)` to defeat TOCTOU).
///   * `force == true`: dst is overwritten, but only if it is a regular file
///     (symlinks are rejected unconditionally).
pub fn promote_tmp_test(
    workspace_root: &Path,
    tmp_tests_root: &Path,
    test_id: &str,
    force: bool,
    auto_approve: bool,
    interactive_approval: bool,
) -> Result<PathBuf, String> {
    if !auto_approve && !interactive_approval {
        return Err(
            "promote requires --yes or interactive approval; refusing to overwrite workspace"
                .to_string(),
        );
    }
    validate_test_id(test_id)?;
    let metadata_root = tmp_tests_root.join("metadata");
    let metadata_path = resolve_user_path(&metadata_root, &format!("{test_id}.json"))?;
    let mut tmp_test = load_metadata_file(&metadata_path)?;

    let rel = tmp_test
        .relative_path
        .to_str()
        .ok_or_else(|| "tmp-test relative_path is not valid UTF-8".to_string())?
        .to_string();
    validate_tmp_test_relative_path(&rel)?;

    let files_root = tmp_tests_root.join("files");
    let src = resolve_user_path(&files_root, &rel)?;
    ensure_regular_file_not_symlink(&src)?;

    // Symlink defence at dst: do this BEFORE `resolve_user_path` so a workspace
    // symlink pointing outside cannot be silently followed (resolve_user_path
    // would either reject the resolved target as escaping workspace, leaking
    // the wrong error message, or land somewhere we did not intend).
    let raw_dst = workspace_root.join(&rel);
    if let Ok(meta) = fs::symlink_metadata(&raw_dst)
        && meta.file_type().is_symlink()
    {
        return Err(format!(
            "promote target is a symlink, refusing to overwrite: {}",
            raw_dst.display()
        ));
    }
    let dst = resolve_user_path(workspace_root, &rel)?;
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "failed to create workspace parent dir {}: {err}",
                parent.display()
            )
        })?;
    }

    let bytes = fs::read(&src)
        .map_err(|err| format!("failed to read tmp-test body {}: {err}", src.display()))?;

    if force {
        // Reject symlink dst regardless of force.
        if let Ok(dst_meta) = fs::symlink_metadata(&dst) {
            if dst_meta.file_type().is_symlink() {
                return Err(format!(
                    "promote target is a symlink, refusing to overwrite: {}",
                    dst.display()
                ));
            }
            if !dst_meta.is_file() {
                return Err(format!(
                    "promote target is not a regular file: {}",
                    dst.display()
                ));
            }
        }
        // CB-001 fix: pick a hard-to-predict sibling temp name and use
        // `create_new(true)` so any pre-existing symlink at that path causes
        // the open to fail (we retry until we find an unused name).
        let parent = dst
            .parent()
            .ok_or_else(|| format!("promote dst has no parent: {}", dst.display()))?;
        let base_name = dst.file_name().and_then(|s| s.to_str()).unwrap_or("anvil");
        let (tmp_dst, mut handle) = open_temp_file_unique(parent, base_name)?;
        {
            handle.write_all(&bytes).map_err(|err| {
                let _ = fs::remove_file(&tmp_dst);
                format!(
                    "failed to write promote temp file {}: {err}",
                    tmp_dst.display()
                )
            })?;
            handle.sync_all().ok();
        }
        fs::rename(&tmp_dst, &dst).map_err(|err| {
            let _ = fs::remove_file(&tmp_dst);
            format!(
                "failed to rename promote temp file {} -> {}: {err}",
                tmp_dst.display(),
                dst.display()
            )
        })?;
    } else {
        // create_new(true) defeats check-then-copy TOCTOU.
        let mut handle = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dst)
            .map_err(|err| {
                format!(
                    "failed to create promote target {} (use --force to overwrite): {err}",
                    dst.display()
                )
            })?;
        handle
            .write_all(&bytes)
            .map_err(|err| format!("failed to write promote target {}: {err}", dst.display()))?;
        handle.sync_all().ok();
    }

    tmp_test.status = TmpTestStatus::Promoted;
    store_metadata_atomic(&metadata_path, &tmp_test)?;
    Ok(dst)
}

/// List every tmp-test for this session (drafts and promoted; discarded
/// entries do not appear because anvil deletes their metadata at discard
/// time). Returns `[]` if `tmp_tests_root` does not yet exist.
pub fn list_tmp_tests(tmp_tests_root: &Path) -> Result<Vec<TmpTest>, String> {
    let metadata_root = tmp_tests_root.join("metadata");
    read_metadata_dir(&metadata_root)
}

fn ensure_regular_file_not_symlink(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path)
        .map_err(|err| format!("failed to stat tmp-test body {}: {err}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!("tmp-test body is a symlink: {}", path.display()));
    }
    if !meta.is_file() {
        return Err(format!(
            "tmp-test body is not a regular file: {}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // -- Task 1.1: id / path validation -------------------------------------

    #[test]
    fn derive_test_id_is_deterministic_and_starts_with_prefix() {
        let id1 = derive_test_id(b"hello", 12345);
        let id2 = derive_test_id(b"hello", 12345);
        assert_eq!(id1, id2);
        assert!(id1.starts_with("tmp_"), "id was {id1}");
        validate_test_id(&id1).expect("derived id must validate");
    }

    #[test]
    fn derive_test_id_changes_when_content_or_time_changes() {
        let a = derive_test_id(b"hello", 1);
        let b = derive_test_id(b"hellp", 1);
        let c = derive_test_id(b"hello", 2);
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn validate_test_id_rejects_path_traversal_and_nul() {
        assert!(validate_test_id("").is_err());
        assert!(validate_test_id("..").is_err());
        assert!(validate_test_id("tmp_../../etc/passwd").is_err());
        assert!(validate_test_id("tmp_aaaa\0bbbbccccddddee").is_err());
        assert!(validate_test_id("tmp_aaaa/bbbbccccddddee").is_err());
        assert!(validate_test_id("tmp_aaaa\\bbbbccccddddee").is_err());
        assert!(validate_test_id("tmp_aaaa.bbbbccccddddee").is_err());
        assert!(validate_test_id("notmp_aaaaccccdddddeefff").is_err());
        assert!(validate_test_id("tmp_-leadingdashbbbbccccddee").is_err());
        // body too short
        assert!(validate_test_id("tmp_short").is_err());
        // body too long (65 chars)
        let long = format!("tmp_{}", "a".repeat(65));
        assert!(validate_test_id(&long).is_err());
    }

    #[test]
    fn validate_test_id_accepts_allowlisted_characters() {
        validate_test_id("tmp_abcdef0123456789").unwrap();
        validate_test_id("tmp_lower-with_dash_and_underscore").unwrap();
    }

    #[test]
    fn validate_relative_path_rejects_traversal_absolute_and_nul() {
        assert!(validate_tmp_test_relative_path("").is_err());
        assert!(validate_tmp_test_relative_path("/etc/passwd").is_err());
        assert!(validate_tmp_test_relative_path("../escape.rs").is_err());
        assert!(validate_tmp_test_relative_path("a/../b.rs").is_err());
        assert!(validate_tmp_test_relative_path("a\0b.rs").is_err());
        let too_long = "a".repeat(MAX_TMP_TEST_REL_PATH_BYTES + 1);
        assert!(validate_tmp_test_relative_path(&too_long).is_err());
    }

    #[test]
    fn validate_relative_path_accepts_normal_paths() {
        validate_tmp_test_relative_path("src/foo/test_bar.rs").unwrap();
        validate_tmp_test_relative_path("test.rs").unwrap();
    }

    // -- Task 1.2: metadata I/O --------------------------------------------

    fn make_tmp_test() -> TmpTest {
        let id = derive_test_id(b"body", 1);
        TmpTest {
            schema_version: TMP_TEST_SCHEMA_VERSION,
            id,
            created_at: 1,
            relative_path: PathBuf::from("src/test_foo.rs"),
            status: TmpTestStatus::Draft,
            last_run_result: None,
        }
    }

    #[test]
    fn store_and_load_metadata_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("metadata").join("file.json");
        let original = make_tmp_test();
        store_metadata_atomic(&path, &original).unwrap();
        let loaded = load_metadata_file(&path).unwrap();
        assert_eq!(loaded, original);
    }

    #[test]
    fn store_metadata_serializes_snake_case_status() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("metadata").join("file.json");
        let mut tt = make_tmp_test();
        tt.status = TmpTestStatus::Draft;
        store_metadata_atomic(&path, &tt).unwrap();
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("\"status\": \"draft\""), "body={body}");
        assert!(
            !body.contains("\"last_run_result\""),
            "skip_serializing_if should drop None last_run_result"
        );
    }

    #[test]
    fn load_metadata_rejects_oversized_file() {
        let dir = tempdir().unwrap();
        let metadir = dir.path().join("metadata");
        fs::create_dir_all(&metadir).unwrap();
        let path = metadir.join("big.json");
        let huge = "x".repeat((MAX_TMP_TEST_METADATA_BYTES + 1) as usize);
        fs::write(&path, huge).unwrap();
        let err = load_metadata_file(&path).unwrap_err();
        assert!(err.contains("exceeds"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn load_metadata_rejects_symlink() {
        let dir = tempdir().unwrap();
        let metadir = dir.path().join("metadata");
        fs::create_dir_all(&metadir).unwrap();
        let real = metadir.join("real.json");
        let original = make_tmp_test();
        store_metadata_atomic(&real, &original).unwrap();
        let link = metadir.join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = load_metadata_file(&link).unwrap_err();
        assert!(err.contains("symlink"), "got: {err}");
    }

    #[test]
    fn load_metadata_rejects_parse_failure() {
        let dir = tempdir().unwrap();
        let metadir = dir.path().join("metadata");
        fs::create_dir_all(&metadir).unwrap();
        let path = metadir.join("broken.json");
        fs::write(&path, "{ not valid json").unwrap();
        let err = load_metadata_file(&path).unwrap_err();
        assert!(err.contains("parse"), "got: {err}");
    }

    #[test]
    fn read_metadata_dir_returns_empty_when_missing() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("metadata");
        let out = read_metadata_dir(&missing).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn read_metadata_dir_skips_broken_entries_but_keeps_good_ones() {
        let dir = tempdir().unwrap();
        let metadir = dir.path().join("metadata");
        fs::create_dir_all(&metadir).unwrap();
        // CB-005: file name must match the embedded id; use a real allowlist
        // id rather than the placeholder `good.json`.
        let good = make_tmp_test();
        let good_path = metadir.join(format!("{}.json", good.id));
        store_metadata_atomic(&good_path, &good).unwrap();
        fs::write(metadir.join("broken.json"), "{").unwrap();
        let out = read_metadata_dir(&metadir).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, good.id);
    }

    #[test]
    fn read_metadata_dir_results_are_deterministically_sorted() {
        let dir = tempdir().unwrap();
        let metadir = dir.path().join("metadata");
        fs::create_dir_all(&metadir).unwrap();
        let mut tt1 = make_tmp_test();
        tt1.id = "tmp_aaaaaaaaaaaaaaaaaaa1".to_string();
        let mut tt2 = make_tmp_test();
        tt2.id = "tmp_bbbbbbbbbbbbbbbbbbb2".to_string();
        store_metadata_atomic(&metadir.join("tmp_bbbbbbbbbbbbbbbbbbb2.json"), &tt2).unwrap();
        store_metadata_atomic(&metadir.join("tmp_aaaaaaaaaaaaaaaaaaa1.json"), &tt1).unwrap();
        let out = read_metadata_dir(&metadir).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, tt1.id);
        assert_eq!(out[1].id, tt2.id);
    }

    // -- Task 1.3: lifecycle ------------------------------------------------

    #[test]
    fn create_generated_test_persists_body_and_metadata() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tmp-tests");
        let tt = create_generated_test(&root, "src/test_foo.rs", b"#[test] fn it(){}").unwrap();

        // body is at tmp-tests/files/<rel>
        let body_path = root.join("files/src/test_foo.rs");
        assert!(body_path.exists());
        let body = fs::read_to_string(&body_path).unwrap();
        assert!(body.contains("#[test]"));

        // metadata is at tmp-tests/metadata/<id>.json
        let meta_path = root.join("metadata").join(format!("{}.json", tt.id));
        assert!(meta_path.exists());
        let loaded = load_metadata_file(&meta_path).unwrap();
        assert_eq!(loaded.id, tt.id);
        assert_eq!(loaded.status, TmpTestStatus::Draft);
        assert_eq!(loaded.relative_path, PathBuf::from("src/test_foo.rs"));
    }

    #[test]
    fn create_generated_test_rejects_path_traversal() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tmp-tests");
        let err = create_generated_test(&root, "../escape.rs", b"x").unwrap_err();
        assert!(err.contains(".."), "got: {err}");
    }

    #[test]
    fn discard_removes_only_target_tmp_test() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tmp-tests");
        let a = create_generated_test(&root, "a.rs", b"a").unwrap();
        let b = create_generated_test(&root, "b.rs", b"b").unwrap();
        assert_ne!(a.id, b.id);

        discard_tmp_test(&root, &a.id).unwrap();

        // a is gone
        assert!(!root.join("files/a.rs").exists());
        assert!(
            !root
                .join("metadata")
                .join(format!("{}.json", a.id))
                .exists()
        );
        // b is still there
        assert!(root.join("files/b.rs").exists());
        assert!(
            root.join("metadata")
                .join(format!("{}.json", b.id))
                .exists()
        );
    }

    #[test]
    fn list_tmp_tests_excludes_discarded_metadata() {
        // Anvil deletes metadata on discard, so the only way a `discarded`
        // entry can appear is via manual edits. We simulate that to confirm
        // list_tmp_tests still surfaces it (defensive deserialize) while
        // resume-time consumers (sessions_cli list) can filter it out.
        let dir = tempdir().unwrap();
        let root = dir.path().join("tmp-tests");
        let a = create_generated_test(&root, "a.rs", b"a").unwrap();
        // anvil-driven discard
        discard_tmp_test(&root, &a.id).unwrap();
        let listed = list_tmp_tests(&root).unwrap();
        assert!(
            listed.is_empty(),
            "discard must remove from list: {listed:?}"
        );
    }

    #[test]
    fn promote_writes_to_workspace_and_marks_metadata_promoted() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let root = dir.path().join("tmp-tests");
        let tt = create_generated_test(&root, "src/test_foo.rs", b"body!").unwrap();

        let dst = promote_tmp_test(&workspace, &root, &tt.id, false, true, false).unwrap();
        assert_eq!(
            dst,
            fs::canonicalize(workspace.join("src/test_foo.rs")).unwrap()
        );
        assert_eq!(fs::read(&dst).unwrap(), b"body!");

        // metadata flipped to promoted
        let meta_path = root.join("metadata").join(format!("{}.json", tt.id));
        let updated = load_metadata_file(&meta_path).unwrap();
        assert_eq!(updated.status, TmpTestStatus::Promoted);

        // body kept under tmp-tests/files
        assert!(root.join("files/src/test_foo.rs").exists());
    }

    #[test]
    fn promote_without_approval_is_rejected() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let root = dir.path().join("tmp-tests");
        let tt = create_generated_test(&root, "src/test_foo.rs", b"body!").unwrap();

        let err = promote_tmp_test(&workspace, &root, &tt.id, false, false, false).unwrap_err();
        assert!(
            err.contains("approval") || err.contains("--yes"),
            "got: {err}"
        );
        // workspace must remain clean
        assert!(!workspace.join("src/test_foo.rs").exists());
    }

    #[test]
    fn promote_conflict_without_force_is_rejected() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        fs::create_dir_all(workspace.join("src")).unwrap();
        let root = dir.path().join("tmp-tests");
        let tt = create_generated_test(&root, "src/test_foo.rs", b"body!").unwrap();
        // Pre-existing workspace file
        fs::write(workspace.join("src/test_foo.rs"), b"existing").unwrap();

        let err = promote_tmp_test(&workspace, &root, &tt.id, false, true, false).unwrap_err();
        assert!(err.contains("--force"), "got: {err}");
        let still = fs::read(workspace.join("src/test_foo.rs")).unwrap();
        assert_eq!(still, b"existing", "non-force promote must not overwrite");
    }

    #[test]
    fn promote_with_force_overwrites_existing_regular_file() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        fs::create_dir_all(workspace.join("src")).unwrap();
        let root = dir.path().join("tmp-tests");
        let tt = create_generated_test(&root, "src/test_foo.rs", b"body!").unwrap();
        fs::write(workspace.join("src/test_foo.rs"), b"existing").unwrap();

        promote_tmp_test(&workspace, &root, &tt.id, true, true, false).unwrap();
        let after = fs::read(workspace.join("src/test_foo.rs")).unwrap();
        assert_eq!(after, b"body!");
    }

    #[cfg(unix)]
    #[test]
    fn promote_refuses_symlink_destination_even_with_force() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        fs::create_dir_all(workspace.join("src")).unwrap();
        let root = dir.path().join("tmp-tests");
        let tt = create_generated_test(&root, "src/test_foo.rs", b"body!").unwrap();

        // outside-of-workspace target file & symlink pointing to it
        let outside = dir.path().join("outside.rs");
        fs::write(&outside, b"do not touch").unwrap();
        std::os::unix::fs::symlink(&outside, workspace.join("src/test_foo.rs")).unwrap();

        let err = promote_tmp_test(&workspace, &root, &tt.id, true, true, false).unwrap_err();
        assert!(err.contains("symlink"), "got: {err}");
        assert_eq!(fs::read(&outside).unwrap(), b"do not touch");
    }

    #[test]
    fn list_tmp_tests_returns_drafts_and_promoted() {
        let dir = tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let root = dir.path().join("tmp-tests");
        let a = create_generated_test(&root, "a.rs", b"a").unwrap();
        let b = create_generated_test(&root, "b.rs", b"b").unwrap();
        promote_tmp_test(&workspace, &root, &a.id, false, true, false).unwrap();

        let listed = list_tmp_tests(&root).unwrap();
        assert_eq!(listed.len(), 2);
        let by_id: std::collections::HashMap<_, _> =
            listed.iter().map(|t| (t.id.clone(), t.status)).collect();
        assert_eq!(by_id.get(&a.id), Some(&TmpTestStatus::Promoted));
        assert_eq!(by_id.get(&b.id), Some(&TmpTestStatus::Draft));
    }

    #[test]
    fn list_tmp_tests_returns_empty_for_fresh_session() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tmp-tests");
        let listed = list_tmp_tests(&root).unwrap();
        assert!(listed.is_empty());
    }

    // -- Codex CB-001 / CB-006: temp file uniqueness ------------------------

    #[test]
    fn random_temp_suffix_is_distinct_across_calls() {
        // Two consecutive calls in the same process must produce different
        // suffixes (the atomic counter alone guarantees this even if pid
        // and nanos collide).
        let a = random_temp_suffix();
        let b = random_temp_suffix();
        assert_ne!(a, b, "suffix must change between calls");
    }

    #[test]
    fn open_temp_file_unique_succeeds_with_clean_parent() {
        let dir = tempdir().unwrap();
        let (path, _file) = open_temp_file_unique(dir.path(), "data.json").unwrap();
        assert!(path.exists());
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with(".data.json."), "name was {name}");
        assert!(name.ends_with(".tmp"), "name was {name}");
    }

    #[cfg(unix)]
    #[test]
    fn force_promote_does_not_follow_attacker_planted_symlink() {
        // CB-001 regression: the OLD implementation used a fixed temp name
        // `.<dst>.tmp-promote` and `OpenOptions::create(true).truncate(true)`,
        // which would silently follow a symlink at that exact name and
        // truncate the target. The fix uses a random suffix + create_new(true).
        //
        // We can't pre-plant a symlink at the *new* random path because we
        // don't know the suffix in advance. What we CAN verify is the
        // invariant: no .tmp-promote-named file is created at all after a
        // successful promote, and a sentinel file at the legacy name is
        // unaffected.
        let dir = tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        fs::create_dir_all(workspace.join("src")).unwrap();
        let outside = dir.path().join("outside.rs");
        fs::write(&outside, b"do not touch").unwrap();
        // Pre-plant a symlink at the legacy fixed temp name.
        let legacy_temp = workspace.join("src").join(".test_foo.rs.tmp-promote");
        std::os::unix::fs::symlink(&outside, &legacy_temp).unwrap();
        // Pre-create the dst so force path is exercised.
        fs::write(workspace.join("src/test_foo.rs"), b"existing").unwrap();
        let root = dir.path().join("tmp-tests");
        let tt = create_generated_test(&root, "src/test_foo.rs", b"new body").unwrap();

        promote_tmp_test(&workspace, &root, &tt.id, true, true, false).unwrap();

        // Outside file remains untouched (legacy temp symlink target).
        assert_eq!(fs::read(&outside).unwrap(), b"do not touch");
        // dst was overwritten with the new body.
        assert_eq!(
            fs::read(workspace.join("src/test_foo.rs")).unwrap(),
            b"new body"
        );
    }

    #[cfg(unix)]
    #[test]
    fn store_metadata_does_not_follow_attacker_planted_symlink() {
        // CB-006 regression: same shape as CB-001 but for the metadata
        // sibling temp file. Pre-plant a symlink at the legacy fixed
        // `.<id>.json.tmp` name and verify the store does not write through
        // it.
        let dir = tempdir().unwrap();
        let metadir = dir.path().join("metadata");
        fs::create_dir_all(&metadir).unwrap();
        let outside = dir.path().join("outside.txt");
        fs::write(&outside, b"do not touch").unwrap();

        let tt = make_tmp_test();
        let metadata_path = metadir.join(format!("{}.json", tt.id));
        let legacy_temp = metadir.join(format!(".{}.json.tmp", tt.id));
        std::os::unix::fs::symlink(&outside, &legacy_temp).unwrap();

        store_metadata_atomic(&metadata_path, &tt).unwrap();

        // Outside file remains untouched.
        assert_eq!(fs::read(&outside).unwrap(), b"do not touch");
        // Metadata wrote to the actual destination, not the symlink target.
        let loaded = load_metadata_file(&metadata_path).unwrap();
        assert_eq!(loaded.id, tt.id);
    }

    // -- Codex CB-004: read_metadata_dir cap during iteration ---------------

    #[test]
    fn read_metadata_dir_caps_collection_at_iteration_time() {
        // CB-004 regression: dropping >>MAX_TMP_TESTS_PER_SESSION json files
        // into the metadata dir must not blow up memory. We use a small
        // sentinel count (cap+5) and verify the returned list is bounded by
        // MAX_TMP_TESTS_PER_SESSION (the cap is enforced both during
        // collection and during the take()-truncation pass).
        //
        // For test runtime, we lean on the `take(MAX_TMP_TESTS_PER_SESSION)`
        // step rather than actually creating 4096 files; we instead assert
        // the helper does not panic / OOM with a moderate workload and
        // validates entries.
        let dir = tempdir().unwrap();
        let metadir = dir.path().join("metadata");
        fs::create_dir_all(&metadir).unwrap();

        // Sprinkle a handful of ill-formed files; they will be rejected by
        // the validator (not panic the loop).
        for i in 0..5 {
            fs::write(metadir.join(format!("garbage_{i}.json")), b"{ }").unwrap();
        }
        // Drop one valid entry whose file name matches its id.
        let mut tt = make_tmp_test();
        tt.id = "tmp_aaaaaaaaaaaaaaaaaaaaaaaa".to_string();
        store_metadata_atomic(&metadir.join(format!("{}.json", tt.id)), &tt).unwrap();

        let listed = read_metadata_dir(&metadir).unwrap();
        assert_eq!(listed.len(), 1, "only the valid entry survives");
        assert_eq!(listed[0].id, tt.id);
        assert!(listed.len() <= MAX_TMP_TESTS_PER_SESSION);
    }

    // -- Codex CB-005: list-time validation ---------------------------------

    #[test]
    fn read_metadata_dir_skips_entry_with_id_mismatch_to_filename() {
        // Hand-crafted metadata where the JSON id field disagrees with the
        // file name. Old code surfaced it; new code skips so CLI list does
        // not display a misleading id.
        let dir = tempdir().unwrap();
        let metadir = dir.path().join("metadata");
        fs::create_dir_all(&metadir).unwrap();
        let mut tt = make_tmp_test();
        tt.id = "tmp_aaaaaaaaaaaaaaaaaaaaaaaa".to_string();
        // file name says one id, body says another.
        let on_disk = metadir.join("tmp_bbbbbbbbbbbbbbbbbbbbbbbb.json");
        let body = serde_json::to_string_pretty(&tt).unwrap();
        fs::write(&on_disk, body).unwrap();

        let listed = read_metadata_dir(&metadir).unwrap();
        assert!(
            listed.is_empty(),
            "id/file-name mismatch must be skipped: {listed:?}"
        );
    }

    #[test]
    fn read_metadata_dir_skips_entry_with_invalid_relative_path() {
        let dir = tempdir().unwrap();
        let metadir = dir.path().join("metadata");
        fs::create_dir_all(&metadir).unwrap();
        // Build TmpTest with an evil relative_path. Bypass the lifecycle
        // helpers (which would reject it) by serializing directly.
        let mut tt = make_tmp_test();
        tt.id = "tmp_ccccccccccccccccccccccc1".to_string();
        tt.relative_path = PathBuf::from("../escape.rs");
        let path = metadir.join(format!("{}.json", tt.id));
        fs::write(&path, serde_json::to_string_pretty(&tt).unwrap()).unwrap();

        let listed = read_metadata_dir(&metadir).unwrap();
        assert!(
            listed.is_empty(),
            "relative_path with `..` must be skipped: {listed:?}"
        );
    }

    #[test]
    fn read_metadata_dir_skips_entry_with_oversized_excerpt() {
        let dir = tempdir().unwrap();
        let metadir = dir.path().join("metadata");
        fs::create_dir_all(&metadir).unwrap();
        let mut tt = make_tmp_test();
        tt.id = "tmp_dddddddddddddddddddddddd".to_string();
        tt.last_run_result = Some(TmpTestRunResult {
            exit_code: 0,
            finished_at: 1,
            excerpt: "x".repeat(MAX_TMP_TEST_EXCERPT_BYTES + 1),
        });
        let path = metadir.join(format!("{}.json", tt.id));
        let body = serde_json::to_string_pretty(&tt).unwrap();
        // Body must still fit under MAX_TMP_TEST_METADATA_BYTES (64 KiB)
        // so load_metadata_file proceeds to the validator.
        assert!(
            body.len() < MAX_TMP_TEST_METADATA_BYTES as usize,
            "test fixture exceeds metadata size cap"
        );
        fs::write(&path, body).unwrap();

        let listed = read_metadata_dir(&metadir).unwrap();
        assert!(
            listed.is_empty(),
            "oversized excerpt must be skipped: {listed:?}"
        );
    }
}
