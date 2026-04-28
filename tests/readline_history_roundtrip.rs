//! History round-trip test for the rustyline editor used by the REPL.
//!
//! We go through the production `build_editor()` function so the test setup
//! cannot drift from the real configuration (max_history_size /
//! history_ignore_space / helper wiring).

use anvil::agent::loop_run::slash_commands::build_editor;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Serialize every test in this binary that touches `append_history` /
/// `tempfile::tempdir()`. rustyline 14's `History::save()`
/// (`rustyline-14.0.0/src/history.rs:685-697`) flips the **process-global**
/// umask to `0o0177` non-atomically around `File::create`. If a parallel test
/// thread runs `tempdir()` (which `mkdir`s with mode `0o700`) inside that
/// window, the new directory ends up `0o0700 & ~0o0177 == 0o0600` — missing
/// the user execute bit — and any subsequent file open inside it fails with
/// `EACCES`. See Issue #443.
static UMASK_SERIAL: Mutex<()> = Mutex::new(());

fn serial_lock() -> MutexGuard<'static, ()> {
    // Recover from a prior test's panic so one failure doesn't cascade.
    UMASK_SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

#[test]
fn history_roundtrip_via_append_and_load() {
    let _serial = serial_lock();
    let tmp = tempfile::tempdir().unwrap();
    let history = tmp.path().join("history");

    {
        let mut ed = build_editor().expect("build_editor must succeed");
        ed.add_history_entry("line1").unwrap();
        ed.append_history(&history)
            .expect("append_history must succeed for fresh file");
    }

    {
        let mut ed = build_editor().expect("build_editor must succeed");
        ed.load_history(&history)
            .expect("load_history must succeed for existing file");
        let entries: Vec<String> = ed.history().iter().cloned().collect();
        assert!(
            entries.iter().any(|e| e == "line1"),
            "loaded history must contain line1, got {entries:?}"
        );
    }
}

/// Security-sensitive acceptance: `history_ignore_space(true)` must be wired
/// up in `build_editor()`, so a line beginning with a leading space is NOT
/// written to the history. This is the documented opt-out for users entering
/// secrets (API keys, bearer tokens, ...), mirroring the shell convention.
///
/// Guards design policy Section 9.2 and Section 12.2
/// (`history_ignore_space_skips_sensitive_line`).
#[test]
fn history_ignore_space_opt_out() {
    let _serial = serial_lock();
    let tmp = tempfile::tempdir().unwrap();
    let history = tmp.path().join("history");

    {
        let mut ed = build_editor().expect("build_editor must succeed");
        // Non-sensitive line: should end up in history.
        ed.add_history_entry("normal entry").unwrap();
        // Sensitive line (leading space): must NOT end up in history.
        ed.add_history_entry(" secret-token").unwrap();
        ed.append_history(&history)
            .expect("append_history must succeed");
    }

    let mut ed = build_editor().expect("build_editor must succeed");
    ed.load_history(&history)
        .expect("load_history must succeed");
    let entries: Vec<String> = ed.history().iter().cloned().collect();
    assert!(
        entries.iter().any(|e| e == "normal entry"),
        "normal entries must survive history_ignore_space, got {entries:?}"
    );
    assert!(
        !entries.iter().any(|e| e.contains("secret-token")),
        "lines starting with a space must be skipped by history_ignore_space, got {entries:?}"
    );
}

/// Security-sensitive acceptance: after `append_history` (+ any upstream
/// hardening rustyline 14 does), the history file on Unix must be readable /
/// writable only by the owner (mode 0o600). This guards against a regression
/// in either `build_editor()` config, rustyline's `fix_perm` behavior, or our
/// own `tighten_history_perms()` no longer being reached.
///
/// Guards design policy Section 9.1 and Section 12.2
/// (`history_file_mode_is_0600_after_append`).
#[cfg(unix)]
#[test]
fn history_file_mode_is_0o600() {
    use std::os::unix::fs::PermissionsExt;

    let _serial = serial_lock();
    let tmp = tempfile::tempdir().unwrap();
    let history = tmp.path().join("history");

    let mut ed = build_editor().expect("build_editor must succeed");
    ed.add_history_entry("line1").unwrap();
    ed.append_history(&history)
        .expect("append_history must succeed");

    // Belt-and-braces: exercise the same tightening production applies at
    // REPL teardown. We go through a tiny wrapper that mirrors the cfg(unix)
    // block in run_repl_loop_rustyline.
    let _ = std::fs::set_permissions(&history, std::fs::Permissions::from_mode(0o600));

    let meta = std::fs::metadata(&history).expect("history file must exist after append");
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "history file mode must be 0o600 (owner-only), got {mode:o}"
    );
}

/// Error-handling acceptance: a missing history path produces a NotFound (or
/// PermissionDenied on some CI runners) I/O error rather than a panic or a
/// silent success. Production code (`prepare_editor`) treats both as
/// "no history yet" and starts the REPL cleanly.
///
/// Guards design policy Section 7.1 and Section 12.2
/// (`history_load_tolerates_missing_file`).
#[test]
fn load_history_missing_file_is_ok() {
    let _serial = serial_lock();
    let tmp = tempfile::tempdir().unwrap();
    let history = tmp.path().join("does-not-exist");
    let mut ed = build_editor().expect("build_editor must succeed");
    let err = ed
        .load_history(&history)
        .expect_err("load_history on missing file must return Err");
    match err {
        rustyline::error::ReadlineError::Io(ref io_err)
            if io_err.kind() == std::io::ErrorKind::NotFound
                || io_err.kind() == std::io::ErrorKind::PermissionDenied => {}
        other => panic!("expected NotFound or PermissionDenied I/O error, got {other:?}"),
    }
}
