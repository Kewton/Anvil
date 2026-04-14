//! Phase 1 tests for the ESC keyboard watcher (Issue #379).
//!
//! Covers:
//! - `KeyboardWatcher::spawn` returns `None` when disabled (non-interactive
//!   or TTY-less CI), guaranteeing a no-op on all CI runners (DR1-006).
//! - `RawModeGuard::inactive()` constructs a no-op guard whose Drop does not
//!   attempt to touch the terminal (safe in TTY-less contexts).
//! - Panic hook installation is idempotent and does not panic on repeat calls
//!   (Task 1.2 / R1 safety invariant).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anvil::tui::{KeyboardWatcher, RawModeGuard, install_raw_mode_panic_hook};

#[test]
fn keyboard_watcher_is_noop_when_disabled() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let watcher = KeyboardWatcher::spawn(Arc::clone(&shutdown), false);
    assert!(
        watcher.is_none(),
        "watcher must be no-op (None) when enabled=false"
    );
    // Shutdown flag stays untouched.
    assert!(!shutdown.load(Ordering::Acquire));
}

#[test]
fn keyboard_watcher_noop_does_not_flip_shutdown_flag() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let watcher = KeyboardWatcher::spawn(Arc::clone(&shutdown), false);
    // Even in stub mode, the `stop()` API must be safe to invoke.
    if let Some(w) = watcher {
        w.stop();
    }

    // Give any hypothetical background work a moment (but we do not spawn
    // anything in no-op mode).
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert!(
        !shutdown.load(Ordering::Acquire),
        "no-op watcher must not mutate shutdown flag"
    );
}

#[test]
fn raw_mode_guard_inactive_drop_is_safe() {
    // An inactive guard must drop without touching the terminal,
    // so it is safe to instantiate unconditionally in TTY-less tests.
    let guard = RawModeGuard::inactive();
    drop(guard);
}

#[test]
fn install_raw_mode_panic_hook_is_idempotent() {
    // Calling the installer twice must not panic. Internally guarded by `Once`.
    install_raw_mode_panic_hook();
    install_raw_mode_panic_hook();
}
