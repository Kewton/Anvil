//! Fixed footer status bar for the REPL / resume loop.
//!
//! See `dev-reports/design/issue-430-fixed-footer-design-policy.md` for the
//! full design. This is the **Phase A skeleton** (issue #430): every public
//! API exists with the eventual signatures, but `acquire` always returns a
//! disabled handle and every method is a no-op. The actual daemon thread,
//! DECSTBM lifecycle, and rendering are deferred to subsequent phases.
//!
//! Parallels `spinner.rs` and `interrupt.rs`: same `OnceLock<Mutex<_>>`
//! single-instance guard pattern, `Drop`-based RAII cleanup, and tracing-only
//! warnings on contention. Intentionally not abstracted across spinner /
//! interrupt: they target different fds (stderr / stdin / stdout) and disable
//! envs (`ANVIL_NO_SPINNER` / `ANVIL_NO_INTERRUPT` / `ANVIL_NO_FOOTER`), so
//! common-traiting would re-introduce the provider abstraction CLAUDE.md
//! forbids (see `interrupt.rs` lead comment for the same rationale).

use std::io::{self, IsTerminal};
use std::sync::{Mutex, OnceLock};

use crate::config::{Config, LogLevel};
use crate::modes::plan_act::ExecutionMode;

/// Process-global single-instance guard for the footer lease (AC15).
///
/// `OnceLock<Mutex<bool>>` rather than a plain `Mutex<bool>` because the
/// initial value must be lazily constructed on first access. The bool inside
/// is the "in use" flag: `true` while a `FooterLease` holds the lease, `false`
/// otherwise. Phase A only sets / clears this bit; later phases will gate the
/// real DECSTBM / panic hook install on it.
static FOOTER_LEASE_HELD: OnceLock<Mutex<bool>> = OnceLock::new();

fn lease_lock() -> &'static Mutex<bool> {
    FOOTER_LEASE_HELD.get_or_init(|| Mutex::new(false))
}

/// RAII lease for the fixed footer. In Phase A this is a thin shell:
/// `acquire` decides whether the footer would be eligible (config / TTY /
/// platform / single-instance) and either claims the global lease bit or
/// hands back a fully-disabled handle. Drop releases the lease bit if it was
/// claimed. The actual worker thread, DECSTBM lifecycle, and panic hook
/// chain land in Phase C.
pub struct FooterLease {
    /// `true` when this lease successfully claimed the global lease bit and
    /// is responsible for releasing it on Drop. `false` for fully no-op
    /// leases (disabled by config / non-TTY / Windows / contention).
    holds_lease_bit: bool,
    handle: FooterHandle,
}

/// Lightweight clonable handle distributed to `Agent` / commands.
///
/// Phase A: `enabled` is always `false`, so every method is a no-op. The
/// shape (Clone + bool) is fixed now so call sites in Phase B-D do not need
/// to be re-plumbed when `Arc<FooterState>` lands.
#[derive(Clone)]
pub struct FooterHandle {
    enabled: bool,
}

/// RAII guard returned by `freeze_for_inference` / `freeze_for_prompt`. Drop
/// re-enables footer rendering. In Phase A the guard carries no state because
/// there is no worker to pause.
pub struct FreezeGuard {
    _enabled: bool,
}

impl FooterLease {
    /// Acquire the process-global footer lease.
    ///
    /// Returns a fully no-op `FooterLease` (with `handle.enabled == false`)
    /// in any of the following cases:
    ///   * `cfg(windows)` — Windows ConPTY DECSTBM support is out of scope
    ///     (DR1-012 / AC11 fallback). One-line `tracing::warn!`.
    ///   * `config.footer == false` — explicit user opt-out.
    ///   * stdout is not a TTY — pipes / files / non-interactive harnesses
    ///     must not receive ANSI noise.
    ///   * the lease is already held — second concurrent acquire never wins
    ///     (AC15). One-line `tracing::warn!`.
    ///
    /// Phase A always returns a disabled handle even on the "happy path" — the
    /// real worker / DECSTBM install lands in later phases.
    pub fn acquire(config: &Config) -> Self {
        // Windows: auto-disable per DR1-012. Logged once to make the fallback
        // observable without spamming.
        #[cfg(windows)]
        {
            tracing::warn!("anvil footer: disabled on Windows (auto-fallback, see issue #430)");
            return Self::disabled_lease();
        }

        #[cfg(not(windows))]
        {
            if !config.footer {
                return Self::disabled_lease();
            }
            if !io::stdout().is_terminal() {
                return Self::disabled_lease();
            }

            // Single-instance gate (AC15). On poison we treat the lease as
            // already held: better to skip the footer than risk fighting
            // another (possibly half-installed) instance.
            let mut held = match lease_lock().lock() {
                Ok(g) => g,
                Err(poisoned) => {
                    tracing::warn!("anvil footer: lease mutex poisoned; skipping footer install");
                    let _ = poisoned;
                    return Self::disabled_lease();
                }
            };
            if *held {
                tracing::warn!(
                    "anvil footer: a footer lease is already active; skipping second acquire"
                );
                return Self::disabled_lease();
            }
            *held = true;
            drop(held);

            // Phase A: even though the footer is "eligible" here, we still
            // hand back a disabled handle. The real worker / DECSTBM install
            // lands in Phase C; we only claim the lease bit so Drop can
            // exercise the release path under test.
            Self {
                holds_lease_bit: true,
                handle: FooterHandle { enabled: false },
            }
        }
    }

    /// Construct a no-op lease that does not own the global lease bit.
    fn disabled_lease() -> Self {
        Self {
            holds_lease_bit: false,
            handle: FooterHandle { enabled: false },
        }
    }

    /// Cheap clone of the handle for distribution to `Agent::new`.
    pub fn handle_clone(&self) -> FooterHandle {
        self.handle.clone()
    }
}

impl Drop for FooterLease {
    fn drop(&mut self) {
        if !self.holds_lease_bit {
            return;
        }
        // Best-effort release; never panic from Drop. On poisoned / missing
        // lock we accept that the lease bit stays set (subsequent acquires
        // will warn and return disabled, which is the safer failure mode).
        if let Ok(mut held) = lease_lock().lock() {
            *held = false;
        }
    }
}

impl FooterHandle {
    /// Construct an always-disabled handle. Used by callers that need to
    /// satisfy the type (e.g. `Agent::new` from `tests/e2e_local_llm.rs` or
    /// the oneshot path) without acquiring a real lease.
    pub fn disabled() -> Self {
        Self { enabled: false }
    }

    /// Whether this handle is connected to a live footer worker. Phase A:
    /// always `false`. Public so call sites can do cheap early-returns when
    /// constructing argument lists.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Publish the per-turn token count. No-op in Phase A.
    pub fn publish_tokens(&self, _tokens: usize) {
        if !self.enabled {
            // Phase A: real publish lands in Phase B alongside `FooterState`.
        }
    }

    /// Publish the mode / log level / yes-mode flags. No-op in Phase A.
    pub fn publish_flags(&self, _mode: ExecutionMode, _log: LogLevel, _yes: bool) {
        if !self.enabled {
            // Phase A: real publish lands in Phase B alongside `FooterState`.
        }
    }

    /// Pause the footer worker for an inference / tool stdout-emitting block.
    /// Returns an RAII guard that thaws on drop. No-op in Phase A.
    pub fn freeze_for_inference(&self) -> FreezeGuard {
        FreezeGuard {
            _enabled: self.enabled,
        }
    }

    /// Pause the footer worker for a rustyline prompt. Returns an RAII guard
    /// that thaws on drop. No-op in Phase A.
    pub fn freeze_for_prompt(&self) -> FreezeGuard {
        FreezeGuard {
            _enabled: self.enabled,
        }
    }
}

impl Drop for FreezeGuard {
    fn drop(&mut self) {
        // Phase A: nothing to thaw. Phase D will signal the worker via
        // Condvar here.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Build a minimal `Config` for unit tests. Uses raw struct construction
    /// rather than `Config::load` because we only care about `footer` here.
    fn config_with_footer(enabled: bool) -> Config {
        Config {
            cwd: PathBuf::from("/tmp"),
            requested_model: None,
            requested_sidecar_model: None,
            ollama_host: "http://127.0.0.1:11434".to_string(),
            context_budget: 24_000,
            max_iterations: 12,
            chat_timeout_secs: 300,
            chat_retries: 2,
            log_level: LogLevel::Info,
            stream: false,
            yes_mode: false,
            fresh_session: false,
            oneshot: false,
            prompt: None,
            state_dir_override: None,
            resume: Default::default(),
            footer: enabled,
        }
    }

    #[test]
    fn disabled_handle_reports_disabled() {
        let h = FooterHandle::disabled();
        assert!(!h.is_enabled());
    }

    #[test]
    fn config_footer_false_returns_disabled_handle() {
        // Even on a TTY, config.footer = false short-circuits to no-op.
        let cfg = config_with_footer(false);
        let lease = FooterLease::acquire(&cfg);
        let h = lease.handle_clone();
        assert!(!h.is_enabled());
        // No lease bit held → Drop is a no-op release path.
        drop(lease);
    }

    #[test]
    fn non_tty_returns_disabled_handle() {
        // Tests run with stdout = pipe (non-TTY) under cargo, so this path
        // is exercised regardless of platform. config.footer = true ensures
        // we pass the config gate and hit the TTY check.
        let cfg = config_with_footer(true);
        let lease = FooterLease::acquire(&cfg);
        let h = lease.handle_clone();
        assert!(
            !h.is_enabled(),
            "cargo test stdout is non-TTY; acquire must return disabled handle"
        );
    }

    #[test]
    fn handle_methods_are_no_ops_for_disabled() {
        let h = FooterHandle::disabled();
        h.publish_tokens(12_345);
        h.publish_flags(ExecutionMode::Plan, LogLevel::Trace, true);
        let _g1 = h.freeze_for_inference();
        let _g2 = h.freeze_for_prompt();
        // No assertion needed: the contract is "no panic, no observable
        // effect". Reaching here without panic is the test.
    }

    #[test]
    fn freeze_guard_drop_is_noop_for_disabled() {
        let h = FooterHandle::disabled();
        {
            let _g = h.freeze_for_inference();
            // guard drops at end of scope
        }
        {
            let _g = h.freeze_for_prompt();
        }
    }

    #[test]
    fn cloned_handle_shares_disabled_state() {
        let h = FooterHandle::disabled();
        let h2 = h.clone();
        assert_eq!(h.is_enabled(), h2.is_enabled());
        assert!(!h2.is_enabled());
    }

    /// Drive the `FOOTER_LEASE_HELD` bit directly. Bypasses `acquire` (which
    /// would short-circuit on non-TTY in cargo's harness) so we can verify
    /// the AC15 mutex-bit semantics that Phase C will rely on. Serialised
    /// via the static lock itself, not a test-local mutex.
    #[test]
    fn lease_bit_is_set_and_cleared_under_simulated_acquire() {
        // Defensive: clear stale state in case another test panicked.
        if let Ok(mut g) = lease_lock().lock() {
            *g = false;
        }

        // Simulate a successful acquire by manually flipping the bit, then
        // building a lease that owns it.
        {
            let mut g = lease_lock().lock().unwrap();
            assert!(!*g, "lease bit must start cleared");
            *g = true;
        }
        let lease = FooterLease {
            holds_lease_bit: true,
            handle: FooterHandle { enabled: false },
        };
        assert!(*lease_lock().lock().unwrap(), "bit must be set while held");
        drop(lease);
        assert!(
            !*lease_lock().lock().unwrap(),
            "Drop must release the lease bit"
        );
    }

    /// AC15 simulation: with the lease bit already set, a second `acquire`
    /// observes contention and returns a no-op lease (does not flip the bit
    /// off in its Drop). We construct the contention manually to avoid
    /// depending on `acquire`'s TTY gate.
    #[test]
    fn second_acquire_under_held_bit_returns_disabled() {
        // Defensive: clear stale state.
        if let Ok(mut g) = lease_lock().lock() {
            *g = false;
        }

        // Pre-set the bit as if a real lease were already alive.
        {
            let mut g = lease_lock().lock().unwrap();
            *g = true;
        }

        let cfg = config_with_footer(true);
        let lease = FooterLease::acquire(&cfg);
        // Whatever path acquire took (TTY-gated in cargo), the contract is
        // the returned lease must NOT own the bit when contention exists.
        assert!(!lease.handle_clone().is_enabled());
        drop(lease);

        // Bit must still be set (the no-op lease did not release it).
        assert!(
            *lease_lock().lock().unwrap(),
            "no-op lease must not release the lease bit on Drop"
        );

        // Cleanup for subsequent tests.
        {
            let mut g = lease_lock().lock().unwrap();
            *g = false;
        }
    }
}
