//! Integration tests for the Phase A footer skeleton (issue #430).
//!
//! Phase A intentionally has no DECSTBM install, no daemon thread, and no
//! ANSI emission, so these tests assert the skeleton-level invariants only:
//!
//!   * `FooterLease::acquire` honours the `config.footer = false` short-circuit.
//!   * `FooterLease::acquire` honours the non-TTY short-circuit (cargo's
//!     test harness runs with stdout = pipe, exercising this path).
//!   * The handle returned in both cases is fully disabled, so any caller
//!     plumbing it through `Agent::new` cannot accidentally emit DECSTBM
//!     escapes.
//!
//! AC9's strict "zero acquire on sessions / oneshot" lands in Phase C; this
//! test file is the placeholder marker for that contract. Spawning the
//! `anvil` binary in-process is deliberately avoided to keep the test
//! offline (no Ollama) and to match the existing `tests/session_cli_tests.rs`
//! convention.

use anvil::agent::loop_run::{
    FooterHandle, FooterLease, acquire_footer_with_terminal_flag_for_test,
};
use anvil::config::{Config, LogLevel};
use anvil::modes::plan_act::ExecutionMode;

fn config_with_footer(enabled: bool) -> Config {
    // `Config` is `#[non_exhaustive]` for crate-external use; only set the
    // fields that matter for this test and let `Default` cover the rest.
    let mut cfg = Config::default();
    cfg.footer = enabled;
    cfg
}

#[test]
fn acquire_with_config_footer_false_returns_disabled() {
    let cfg = config_with_footer(false);
    let lease = FooterLease::acquire(&cfg);
    assert!(!lease.handle_clone().is_enabled());
}

#[test]
fn acquire_under_cargo_non_tty_returns_disabled() {
    // Drive the non-TTY branch deterministically instead of assuming cargo's
    // integration-test stdout is always non-terminal in this environment.
    let cfg = config_with_footer(true);
    let lease = acquire_footer_with_terminal_flag_for_test(&cfg, false);
    assert!(!lease.handle_clone().is_enabled());
}

#[test]
fn disabled_handle_current_cols_is_none() {
    let handle = FooterHandle::disabled();
    assert_eq!(handle.current_cols(), None);
}

#[test]
fn disabled_handle_publish_and_freeze_are_noop() {
    // Phase A "AC9 placeholder": the handle plumbed through `Agent::new`
    // along the sessions / oneshot paths must be observably inert. We can't
    // intercept stdout in-process without a PTY, but we can prove the
    // public API contract: nothing panics and no state visibly changes.
    let h = FooterHandle::disabled();
    h.publish_tokens(0);
    h.publish_tokens(usize::MAX);
    h.publish_flags(ExecutionMode::Plan, LogLevel::Info, false);
    h.publish_flags(ExecutionMode::Act, LogLevel::Trace, true);

    {
        let _g = h.freeze_for_inference();
        // guard drops at scope end
    }
    {
        let _g = h.freeze_for_prompt();
    }
}
