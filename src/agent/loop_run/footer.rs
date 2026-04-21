//! Fixed footer status bar for the REPL / resume loop.
//!
//! See `dev-reports/design/issue-430-fixed-footer-design-policy.md` for the
//! full design. **Phase C** (issue #430) lands the daemon thread, DECSTBM
//! lifecycle, panic hook chain, and per-process single-instance enforcement.
//! Phase D will add the spinner / rustyline freeze rendezvous; Phase E will
//! finalise the AC17 self-disable + PTY E2E.
//!
//! Parallels `spinner.rs` and `interrupt.rs`: same `OnceLock<Mutex<_>>`
//! single-instance guard pattern, `Drop`-based RAII cleanup, and tracing-only
//! warnings on contention. Intentionally not abstracted across spinner /
//! interrupt: they target different fds (stderr / stdin / stdout) and disable
//! envs (`ANVIL_NO_SPINNER` / `ANVIL_NO_INTERRUPT` / `ANVIL_NO_FOOTER`), so
//! common-traiting would re-introduce the provider abstraction CLAUDE.md
//! forbids (see `interrupt.rs` lead comment for the same rationale).

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::config::{Config, LogLevel};
use crate::modes::plan_act::ExecutionMode;

/// Pure ANSI escape builders. SSOT for the footer's terminal control strings
/// (DR1-008). Kept side-effect free so unit tests can `assert_eq!` exact byte
/// sequences without spinning up a worker thread or touching real stdout.
///
/// All public functions in this module return `String` / `&'static str` only;
/// no IO. The Phase C worker writes the results to stdout under the freeze
/// rendezvous protocol (Phase D). Anomalous terminal sizes (rows < 2) collapse
/// to `None` from `build_decstbm` so the worker can self-disable rather than
/// emit a malformed `\x1b[1;0r` that some terminals honour by clipping the
/// scroll region to a single line.
pub(super) mod ansi {
    /// Build a DECSTBM (Set Top and Bottom Margins) escape that reserves the
    /// last screen row for the footer.
    ///
    /// `rows` is the current terminal height in rows (1-indexed). The scroll
    /// region is set to lines `1..=top` where `top = rows - 1`, leaving row
    /// `rows` for the fixed footer. `rows < 2` returns `None` because we
    /// cannot reserve a footer row without leaving zero scroll lines.
    pub(crate) fn build_decstbm(rows: u16) -> Option<String> {
        if rows < 2 {
            return None;
        }
        let top = rows - 1;
        Some(format!("\x1b[1;{top}r"))
    }

    /// DECSTBM reset: clears any custom scroll region back to the full screen.
    /// Always safe to send (terminals ignore it when no region is set).
    pub(crate) fn build_decstbm_reset() -> &'static str {
        "\x1b[r"
    }

    /// Cursor Position (CUP). 1-indexed `row;col` per ECMA-48. Callers are
    /// responsible for clamping `row` / `col` to the current terminal size.
    pub(crate) fn move_to(row: u16, col: u16) -> String {
        format!("\x1b[{row};{col}H")
    }

    /// Save cursor (DECSC). Pairs with `restore_cursor`. Used to bracket the
    /// footer redraw so the user-visible cursor in the scrolling region does
    /// not jump.
    pub(crate) fn save_cursor() -> &'static str {
        "\x1b[s"
    }

    /// Restore cursor (DECRC). Counterpart to `save_cursor`.
    pub(crate) fn restore_cursor() -> &'static str {
        "\x1b[u"
    }

    /// Carriage return + erase entire line. Matches the `spinner.rs` clear
    /// pattern (`\r\x1b[2K`) so footer / spinner residue clear identically.
    pub(crate) fn clear_line() -> &'static str {
        "\r\x1b[2K"
    }
}

/// Per-render-cycle snapshot of the footer state. Built by the worker thread
/// under the state's atomics + mutex, then handed to the pure
/// `build_footer_line` function. Keeping it `Copy` lets render code stay
/// trivially testable.
#[derive(Debug, Clone, Copy)]
pub(super) struct FooterSnapshot {
    pub tokens: usize,
    pub budget: usize,
    pub mode: ExecutionMode,
    pub log: LogLevel,
    pub yes: bool,
    /// Cached `tokens / budget` ratio. Worker computes this once so the
    /// renderer does not divide on every call.
    pub ratio: f64,
}

/// Bag of mode / log-level / yes flags published from the REPL command
/// handlers via `FooterHandle::publish_flags`. `Copy` because all fields are
/// trivially-copyable enums / bool.
#[derive(Debug, Clone, Copy)]
pub(super) struct FooterFlags {
    pub mode: ExecutionMode,
    pub log: LogLevel,
    pub yes: bool,
}

impl FooterFlags {
    pub(super) fn new(mode: ExecutionMode, log: LogLevel, yes: bool) -> Self {
        Self { mode, log, yes }
    }
}

impl Default for FooterFlags {
    /// Defaults match `Config::default()` semantics: Act mode (matches
    /// `ModeState::default`), Info log level, no auto-approve.
    fn default() -> Self {
        Self {
            mode: ExecutionMode::Act,
            log: LogLevel::Info,
            yes: false,
        }
    }
}

/// Shared state behind `FooterHandle`. Owned via `Arc` by the daemon thread
/// (Phase C) and by every clone of `FooterHandle`.
///
/// Field invariants:
///   * `budget` is set once at acquire time and never mutated.
///   * `tokens` is `Relaxed` because ±1 token between turns is invisible in a
///     bar with 8 cells.
///   * `flags` uses a `Mutex` (not bit-packing) per DR1-001; collisions are
///     rare (slash command frequency).
///   * `freeze` and `self_disabled` are simple `AtomicBool` flags read by the
///     worker. `self_disabled` is set permanently when the writer rejects the
///     output (broken pipe / detached terminal). `freeze` is wired by Phase D.
pub(super) struct FooterStateInner {
    pub tokens: AtomicUsize,
    pub budget: usize,
    pub flags: Mutex<FooterFlags>,
    pub freeze: AtomicBool,
    pub self_disabled: AtomicBool,
    /// Wake `Condvar` shared with the daemon worker. `freeze_for_inference` /
    /// `freeze_for_prompt` flip `freeze` and `notify_all()` here so the worker
    /// observes the change immediately instead of waiting for the next 200ms
    /// tick. The `Drop` of `FooterLease` and `FreezeGuard` also notify here.
    pub wake: (Mutex<()>, Condvar),
    /// Fallback row count used by `render_loop` when `crossterm::terminal::size()`
    /// returns `Err` (typically non-TTY environments such as CI test harnesses).
    /// `0` means no fallback configured. `install_active` stores the rows it
    /// resolved at acquire time so the worker can keep rendering through the
    /// captured writer even when the live terminal size is unavailable.
    pub fallback_rows: AtomicU16,
}

impl FooterStateInner {
    pub(super) fn new(budget: usize, flags: FooterFlags) -> Arc<Self> {
        Arc::new(Self {
            tokens: AtomicUsize::new(0),
            budget,
            flags: Mutex::new(flags),
            freeze: AtomicBool::new(false),
            self_disabled: AtomicBool::new(false),
            wake: (Mutex::new(()), Condvar::new()),
            fallback_rows: AtomicU16::new(0),
        })
    }

    /// Compute the current footer snapshot. Pure read; safe to call from the
    /// worker thread or tests. Uses `Relaxed` for token reads (see field
    /// invariants).
    pub(super) fn snapshot(&self) -> FooterSnapshot {
        let tokens = self.tokens.load(Ordering::Relaxed);
        let flags = match self.flags.lock() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(), // poisoned → still readable
        };
        let ratio = if self.budget == 0 {
            0.0
        } else {
            tokens as f64 / self.budget as f64
        };
        FooterSnapshot {
            tokens,
            budget: self.budget,
            mode: flags.mode,
            log: flags.log,
            yes: flags.yes,
            ratio,
        }
    }
}

/// Render the entire footer line from a snapshot. Pure function: same input
/// always produces the same bytes, no IO. Phase B's snapshot test (Issue #430
/// body example) compares the output of this function to a literal string.
///
/// Format (DR2-004): `[mode]  bar (NN.Nk / NNk)  [verbose|trace?]  [yes?]`
/// where the separator between every cluster is **two spaces** (Issue body).
/// `bar` carries its own `NN%` suffix via `bar_for_ratio`; the token tuple is
/// appended here so that `bar_for_ratio` stays reusable for tests that only
/// care about the bar visual.
pub(super) fn build_footer_line(snapshot: &FooterSnapshot, use_color: bool) -> String {
    let mode_label = match snapshot.mode {
        ExecutionMode::Plan => "[plan]",
        ExecutionMode::Act => "[act]",
    };
    let bar = bar_for_ratio(snapshot.ratio, use_color);
    let token_tuple = format!(
        "({} / {})",
        format_token_count(snapshot.tokens),
        format_token_count(snapshot.budget)
    );

    // Use a Vec + join("  ") so future flags add a single push() call, not a
    // new format!() permutation (DR1-009 OCP-by-omission).
    let mut parts: Vec<String> = Vec::with_capacity(4);
    parts.push(mode_label.to_string());
    parts.push(format!("{bar} {token_tuple}"));
    match snapshot.log {
        LogLevel::Info => {} // nothing
        LogLevel::Verbose => parts.push("[verbose]".to_string()),
        LogLevel::Trace => parts.push("[trace]".to_string()),
    }
    if snapshot.yes {
        parts.push("[yes]".to_string());
    }
    parts.join("  ")
}

/// Render the 8-cell token usage bar with optional color escalation.
///
/// Cell semantics:
///   * 8 cells total, filled count = `floor(ratio.clamp(0, 1) * 8)`.
///   * `0.0` → empty bar (`░░░░░░░░ 0%`).
///   * `0.42` → `███░░░░░ 42%` (3 = floor(0.42 * 8)).
///   * `>= 0.9 && < 1.0` → wrap in yellow when `use_color`, suffix `NN%`.
///   * `>= 1.0` → wrap in red when `use_color`, suffix `100%` exactly at 1.0
///     and `>100%` for anything strictly above (AC8). Filled cells saturate
///     at 8.
///
/// `use_color=false` (NO_COLOR env) suppresses ANSI escapes entirely so the
/// rendered bar is safe to ship to dumb terminals or capture into snapshots.
pub(super) fn bar_for_ratio(ratio: f64, use_color: bool) -> String {
    const CELLS: usize = 8;
    const FULL: char = '█';
    const EMPTY: char = '░';
    const YELLOW: &str = "\x1b[33m";
    const RED: &str = "\x1b[31m";
    const RESET: &str = "\x1b[0m";

    // Treat NaN / negative as 0.0 so the bar never panics on garbage input.
    let r = if ratio.is_nan() || ratio < 0.0 {
        0.0
    } else {
        ratio
    };
    let clamped = r.min(1.0);
    let filled = (clamped * CELLS as f64).floor() as usize;
    let filled = filled.min(CELLS);
    let empty = CELLS - filled;

    let mut bar = String::with_capacity(CELLS + 8);
    for _ in 0..filled {
        bar.push(FULL);
    }
    for _ in 0..empty {
        bar.push(EMPTY);
    }

    let percent = if r > 1.0 {
        " >100%".to_string()
    } else {
        // Round to nearest integer for display; bar visual uses floor, but the
        // numeric label is friendlier when rounded (e.g. 0.499 -> 50%).
        format!(" {}%", (r * 100.0).round() as i64)
    };

    let body = format!("{bar}{percent}");
    if !use_color {
        return body;
    }
    if r >= 1.0 {
        format!("{RED}{body}{RESET}")
    } else if r >= 0.9 {
        format!("{YELLOW}{body}{RESET}")
    } else {
        body
    }
}

/// Format a token count as Issue-body-style `NN.Nk` / `NNk` / `NNN`.
///
/// Rules (Issue body example: `10.1k`, `24k`):
///   * `0`           → `"0"`
///   * `< 1_000`     → bare integer (`"500"`)
///   * `< 10_000`    → `"N.Nk"` (one decimal, e.g. `1234` → `"1.2k"`)
///   * `>= 10_000`   → `"NN.Nk"` for non-multiples of 1000, drop `.0` for
///     exact thousands (`24_000` → `"24k"`, `10_100` → `"10.1k"`).
fn format_token_count(n: usize) -> String {
    if n == 0 {
        return "0".to_string();
    }
    if n < 1_000 {
        return n.to_string();
    }
    // Round down to the nearest 100 so `10_149` → `10.1k`, not `10.15k`.
    let tenths = n / 100; // e.g. 10100 -> 101, 24000 -> 240
    let whole = tenths / 10; // e.g. 10, 24
    let frac = tenths % 10; // e.g. 1, 0
    if frac == 0 {
        format!("{whole}k")
    } else {
        format!("{whole}.{frac}k")
    }
}

/// Process-global single-instance guard for the footer lease (AC15).
///
/// `OnceLock<Mutex<bool>>` rather than a plain `Mutex<bool>` because the
/// initial value must be lazily constructed on first access. The bool inside
/// is the "in use" flag: `true` while a `FooterLease` holds the lease, `false`
/// otherwise.
static FOOTER_LEASE_HELD: OnceLock<Mutex<bool>> = OnceLock::new();

fn lease_lock() -> &'static Mutex<bool> {
    FOOTER_LEASE_HELD.get_or_init(|| Mutex::new(false))
}

/// Lock the lease bit, recovering from poisoning. Returns `Some(guard)` when
/// the lock was acquired (clean or poisoned-recovered) and `None` only on
/// catastrophic failure (impossible for `Mutex<bool>`).
fn lock_lease_bit_or_recover<'a>() -> std::sync::MutexGuard<'a, bool> {
    match lease_lock().lock() {
        Ok(g) => g,
        Err(poisoned) => {
            // A previous holder panicked while the bit was held. The bit value
            // itself is still valid (Mutex<bool> never observes torn writes),
            // so recover it via `into_inner()` and continue. We log once so
            // the recovery is observable in production.
            tracing::warn!("anvil footer: lease mutex poisoned; recovering inner value");
            poisoned.into_inner()
        }
    }
}

/// Worker tick interval. Matches the work plan (200ms): low enough to satisfy
/// the AC3 "<= 500ms resize follow" budget while keeping CPU near-idle.
const TICK: Duration = Duration::from_millis(200);

/// Box<dyn Write + Send> alias used for both the real stdout writer and the
/// in-memory writer that tests inject through `acquire_for_test`.
type FooterWriter = Box<dyn Write + Send>;

/// Trampoline for the panic hook: stores the chained previous hook so the
/// custom hook can call it after writing the DECSTBM reset. We hold it in an
/// `Arc` because the closure captured by `set_hook` and the `FooterLease`
/// itself both need access (Drop must restore the prev hook).
type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

/// All resources owned by an active footer lease. Constructed by `acquire`
/// (and test variants) and dropped in reverse order by `Drop for FooterLease`.
///
/// The `state` field is intentionally retained even though `Drop` does not
/// read it: the worker holds its own `Arc` clone and the `FooterHandle` clones
/// distributed to `Agent` hold theirs, so this `Arc` is part of the reference
/// count that keeps the shared state alive for the lease's lifetime. Storing
/// it here also gives the future `freeze_for_inference` (Phase D) a place to
/// reach the state without going through the handle.
struct Active {
    state: Arc<FooterStateInner>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    /// Saved previous panic hook so we can restore it on Drop. `Some` after
    /// install; `None` after Drop has consumed it (or if install was skipped).
    prev_panic_hook: Option<Arc<PanicHook>>,
    /// DECSTBM rows recorded at install. Used by Drop to send a reset and a
    /// final `move_to(rows, 1)` so the cursor lands on a clean line below the
    /// scroll region.
    decstbm_rows: u16,
    /// Writer used for DECSTBM set/reset and worker output. The real path
    /// uses `Box::new(io::stdout())`; tests inject an in-memory writer.
    /// Wrapped in an `Arc<Mutex<>>` so the worker thread and the Drop path
    /// can both write through the same underlying handle.
    writer: Arc<Mutex<FooterWriter>>,
}

/// RAII lease for the fixed footer. **Phase C**: when `acquire` lands on the
/// happy path it claims the lease bit, installs the DECSTBM scroll region,
/// chains a panic hook, and spawns the daemon thread. Drop tears down all of
/// that in reverse order. When the env / TTY / config gates fail (or another
/// lease is already held), `acquire` returns a fully no-op lease whose
/// `handle.enabled == false`.
pub struct FooterLease {
    /// `true` when this lease successfully claimed the global lease bit and
    /// is responsible for releasing it on Drop. `false` for fully no-op
    /// leases (disabled by config / non-TTY / Windows / contention).
    holds_lease_bit: bool,
    /// Resources owned by an enabled lease. `None` for disabled leases.
    inner: Option<Active>,
    handle: FooterHandle,
}

/// Lightweight clonable handle distributed to `Agent` / commands.
///
/// `enabled = true` only when the owning lease successfully spawned the
/// daemon thread (Phase C+). `state` carries the shared `FooterStateInner`
/// whenever the handle is wired to one (publish writes always go through
/// when state is attached, regardless of `enabled` — this keeps tests and
/// the Phase B publish API simple).
#[derive(Clone)]
pub struct FooterHandle {
    enabled: bool,
    /// Inner shared state. `None` for fully-disabled handles; `Some` when a
    /// `FooterLease` (or test) opted to attach state. Cheap `Arc::clone` on
    /// `FooterHandle::clone` keeps `Agent` distribution allocation-free.
    state: Option<Arc<FooterStateInner>>,
}

/// RAII guard returned by `freeze_for_inference` / `freeze_for_prompt`. Drop
/// re-enables footer rendering by clearing the `freeze` flag and notifying the
/// daemon worker via the wake `Condvar` carried inside `FooterStateInner`.
/// `state` is `Some` only when the originating handle was wired to a live
/// `FooterStateInner` (i.e. `acquire` succeeded); disabled handles produce a
/// guard with `state == None` so Drop is a no-op.
pub struct FreezeGuard {
    state: Option<Arc<FooterStateInner>>,
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
    ///   * `terminal::size()` fails or returns rows < 2 — auto-disable
    ///     (DR4-001).
    ///   * worker thread spawn or DECSTBM write fails — auto-disable.
    pub fn acquire(config: &Config) -> Self {
        // Windows: auto-disable per DR1-012. Logged once to make the fallback
        // observable without spamming.
        #[cfg(windows)]
        {
            tracing::warn!("anvil footer: disabled on Windows (auto-fallback, see issue #430)");
            let _ = config; // silence unused-var on Windows
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

            // Single-instance gate (AC15). Acquired here and held until `Drop`.
            // We don't keep the guard alive across the whole acquire path — we
            // only flip the bit, then drop the guard so other code that needs
            // to inspect the bit (defensive checks, future audits) is not
            // blocked.
            {
                let mut held = lock_lease_bit_or_recover();
                if *held {
                    tracing::warn!(
                        "anvil footer: a footer lease is already active; skipping second acquire"
                    );
                    return Self::disabled_lease();
                }
                *held = true;
            }

            // Past this point, releasing the lease bit on any failure path is
            // the responsibility of `release_lease_bit_on_failure`. We use an
            // explicit guard struct rather than scattering manual cleanup so
            // a future panic during install also releases the bit.
            let bit_guard = LeaseBitGuard::new();

            // Determine current terminal size. Failure or anomalous values
            // (rows < 2, cols == 0) → auto-disable.
            let rows = match crossterm::terminal::size() {
                Ok((cols, rows)) if cols > 0 && rows >= 2 => rows,
                Ok(_) => {
                    tracing::warn!(
                        "anvil footer: anomalous terminal size; disabling footer install"
                    );
                    drop(bit_guard);
                    return Self::disabled_lease();
                }
                Err(err) => {
                    tracing::warn!(?err, "anvil footer: terminal::size() failed; disabling");
                    drop(bit_guard);
                    return Self::disabled_lease();
                }
            };

            let budget = config.context_budget;
            let initial_flags = FooterFlags {
                mode: ExecutionMode::Act,
                log: config.log_level,
                yes: config.yes_mode,
            };
            let writer: FooterWriter = Box::new(io::stdout());
            match install_active(rows, budget, initial_flags, writer, bit_guard) {
                Ok(lease) => lease,
                Err(()) => {
                    // install_active already released the bit and logged on
                    // failure; return a fresh disabled lease.
                    Self::disabled_lease()
                }
            }
        }
    }

    /// Construct a no-op lease that does not own the global lease bit.
    fn disabled_lease() -> Self {
        Self {
            holds_lease_bit: false,
            inner: None,
            handle: FooterHandle {
                enabled: false,
                state: None,
            },
        }
    }

    /// Cheap clone of the handle for distribution to `Agent::new`.
    pub fn handle_clone(&self) -> FooterHandle {
        self.handle.clone()
    }

    /// Test-only: install an active footer lease with an injected writer and
    /// terminal row count. Bypasses the TTY / config / Windows gates that the
    /// public `acquire` enforces, so unit tests can drive the worker without a
    /// live terminal. The lease bit is still honoured (caller must ensure
    /// the bit is released or use this through the same single-instance
    /// contract as `acquire`).
    #[cfg(test)]
    pub(super) fn acquire_for_test(rows: u16, writer: FooterWriter) -> Self {
        // Claim the bit (or refuse, returning a disabled lease for parity
        // with the public path). Tests serialise via a test-local mutex.
        {
            let mut held = lock_lease_bit_or_recover();
            if *held {
                tracing::warn!("anvil footer (test): lease already held; returning disabled lease");
                return Self::disabled_lease();
            }
            *held = true;
        }
        let bit_guard = LeaseBitGuard::new();
        match install_active(rows, 24_000, FooterFlags::default(), writer, bit_guard) {
            Ok(lease) => lease,
            Err(()) => Self::disabled_lease(),
        }
    }
}

/// Helper: builds and installs the daemon thread, panic hook chain, and
/// DECSTBM. On any failure (write error, spawn error) it releases the lease
/// bit through `bit_guard.release()` and returns `Err(())`; on success the
/// guard is consumed and ownership of the bit transfers to the returned
/// `FooterLease`.
fn install_active(
    rows: u16,
    budget: usize,
    initial_flags: FooterFlags,
    writer: FooterWriter,
    bit_guard: LeaseBitGuard,
) -> Result<FooterLease, ()> {
    // 1. Build the shared state and synchronisation primitives up front.
    //    The wake `Condvar` lives inside `FooterStateInner` (Phase D) so
    //    `FreezeGuard::Drop` can wake the worker through the handle.
    let state = FooterStateInner::new(budget, initial_flags);
    state.fallback_rows.store(rows, Ordering::Relaxed);
    let stop = Arc::new(AtomicBool::new(false));
    let writer = Arc::new(Mutex::new(writer));

    // 2. Set DECSTBM (Task C.4). Failure → release bit and disable.
    let decstbm = match ansi::build_decstbm(rows) {
        Some(s) => s,
        None => {
            tracing::warn!("anvil footer: build_decstbm returned None; disabling");
            bit_guard.release();
            return Err(());
        }
    };
    {
        let mut w = writer
            .lock()
            .expect("footer writer mutex never poisoned at install");
        if let Err(err) = w.write_all(decstbm.as_bytes()) {
            tracing::warn!(?err, "anvil footer: failed to write DECSTBM; disabling");
            bit_guard.release();
            return Err(());
        }
        let _ = w.flush();
    }

    // 3. Install panic hook chain (Task C.2). The custom hook sends the
    //    DECSTBM reset to stdout (best-effort, broken pipes ignored) and then
    //    delegates to `prev` so backtraces and tracing-subscriber's own hook
    //    keep working (AC10).
    let prev_panic_hook: PanicHook = std::panic::take_hook();
    let prev_arc = Arc::new(prev_panic_hook);
    let prev_for_hook = prev_arc.clone();
    std::panic::set_hook(Box::new(move |info| {
        // Best-effort DECSTBM reset directly to raw stdout. We intentionally
        // do NOT touch the FOOTER_LEASE_HELD mutex here (DR2-012 invariant)
        // to avoid deadlocking with the Drop path that may also be running.
        let _ = io::stdout().write_all(b"\x1b[r");
        let _ = io::stdout().flush();
        prev_for_hook(info);
    }));

    // 4. Spawn the worker thread (Task C.3).
    let state_for_thread = state.clone();
    let stop_for_thread = stop.clone();
    let writer_for_thread = writer.clone();
    let spawn_result = thread::Builder::new()
        .name("anvil-footer".into())
        .spawn(move || {
            // catch_unwind so a panic inside the render loop does not abort
            // the process; the panic hook chain still fires before this
            // catch unwinds. Same shape as spinner.rs / interrupt.rs.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                render_loop(state_for_thread, stop_for_thread.clone(), writer_for_thread);
            }));
            stop_for_thread.store(true, Ordering::SeqCst);
        });

    let worker = match spawn_result {
        Ok(h) => h,
        Err(err) => {
            tracing::warn!(?err, "anvil footer: failed to spawn worker; disabling");
            // Reverse panic hook install before bailing.
            std::panic::set_hook(panic_hook_from_arc(prev_arc));
            // Best-effort DECSTBM reset.
            if let Ok(mut w) = writer.lock() {
                let _ = w.write_all(ansi::build_decstbm_reset().as_bytes());
                let _ = w.flush();
            }
            bit_guard.release();
            return Err(());
        }
    };

    // 5. Construct the lease. The bit ownership transfers from `bit_guard`
    //    into `holds_lease_bit = true` here.
    bit_guard.into_owned();
    let handle = FooterHandle {
        enabled: true,
        state: Some(state.clone()),
    };
    Ok(FooterLease {
        holds_lease_bit: true,
        inner: Some(Active {
            state,
            stop,
            worker: Some(worker),
            prev_panic_hook: Some(prev_arc),
            decstbm_rows: rows,
            writer,
        }),
        handle,
    })
}

/// Reconstruct a `Box<dyn Fn>` from the shared `Arc<PanicHook>` so we can call
/// `std::panic::set_hook` (which requires a `Box`). The resulting box closes
/// over the `Arc` and forwards each invocation to the inner hook.
fn panic_hook_from_arc(
    arc: Arc<PanicHook>,
) -> Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static> {
    Box::new(move |info| arc(info))
}

/// RAII helper for the lease bit: ensures the bit is cleared if a failure
/// path drops the guard before transferring ownership to a `FooterLease`.
/// `into_owned` consumes the guard without releasing the bit (used on the
/// success path).
struct LeaseBitGuard {
    armed: bool,
}

impl LeaseBitGuard {
    fn new() -> Self {
        Self { armed: true }
    }

    /// Disarm the guard; caller takes ownership of the bit (and is responsible
    /// for clearing it on Drop).
    fn into_owned(mut self) {
        self.armed = false;
    }

    /// Explicit release (used on early-return failure paths).
    fn release(mut self) {
        if self.armed {
            let mut held = lock_lease_bit_or_recover();
            *held = false;
            self.armed = false;
        }
    }
}

impl Drop for LeaseBitGuard {
    fn drop(&mut self) {
        if self.armed {
            let mut held = lock_lease_bit_or_recover();
            *held = false;
        }
    }
}

impl Drop for FooterLease {
    fn drop(&mut self) {
        // Order matters: stop worker → join → DECSTBM reset → restore panic
        // hook → release lease bit. This mirrors the install order in reverse
        // and matches the design (§3 Drop comment).
        if let Some(mut active) = self.inner.take() {
            // 1. Signal stop and wake the worker via the shared state's wake
            //    Condvar (Phase D consolidates wake into FooterStateInner).
            active.stop.store(true, Ordering::SeqCst);
            {
                let (lock, cvar) = &active.state.wake;
                let _g = lock.lock().unwrap_or_else(|p| p.into_inner());
                cvar.notify_all();
            }
            // 2. Join the worker (best-effort, never panic from Drop).
            if let Some(handle) = active.worker.take()
                && let Err(e) = handle.join()
            {
                tracing::warn!(?e, "anvil footer: worker thread join failed");
            }
            // 3. DECSTBM reset + cursor restore. Best-effort: broken pipes
            //    after the worker disabled itself are ignored.
            if let Ok(mut w) = active.writer.lock() {
                let _ = w.write_all(ansi::build_decstbm_reset().as_bytes());
                let cursor_home = ansi::move_to(active.decstbm_rows, 1);
                let _ = w.write_all(cursor_home.as_bytes());
                let _ = w.flush();
            }
            // 4. Restore the previous panic hook.
            if let Some(prev) = active.prev_panic_hook.take() {
                std::panic::set_hook(panic_hook_from_arc(prev));
            }
        }

        // 5. Release the lease bit (always, even for inner=None disabled
        //    leases that were constructed from `acquire_for_test` failure).
        if self.holds_lease_bit {
            let mut held = lock_lease_bit_or_recover();
            *held = false;
            self.holds_lease_bit = false;
        }
    }
}

impl FooterHandle {
    /// Construct an always-disabled handle. Used by callers that need to
    /// satisfy the type (e.g. `Agent::new` from `tests/e2e_local_llm.rs` or
    /// the oneshot path) without acquiring a real lease.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            state: None,
        }
    }

    /// Whether this handle is connected to a live footer worker. Public so
    /// call sites can do cheap early-returns when constructing argument lists.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Publish the per-turn token count.
    ///
    /// Writes through to `FooterStateInner::tokens` whenever a state is
    /// attached, **regardless of `enabled`**. Phase B's whole point is to let
    /// the publish API land alongside `turn.rs::handle_user_message` and the
    /// slash-command handlers without waiting for the daemon thread.
    pub fn publish_tokens(&self, tokens: usize) {
        if let Some(state) = &self.state {
            state.tokens.store(tokens, Ordering::Relaxed);
        }
    }

    /// Publish the mode / log level / yes-mode flags. Recovers from a
    /// poisoned mutex (footer is fail-open per AC17, never panic from a
    /// publish call site).
    pub fn publish_flags(&self, mode: ExecutionMode, log: LogLevel, yes: bool) {
        let Some(state) = &self.state else {
            return;
        };
        let next = FooterFlags::new(mode, log, yes);
        match state.flags.lock() {
            Ok(mut g) => *g = next,
            Err(p) => *p.into_inner() = next,
        }
    }

    /// Pause the footer worker for an inference / tool stdout-emitting block.
    /// Flips the shared `freeze` flag and notifies the daemon worker so it
    /// stops re-rendering until the returned `FreezeGuard` drops.
    ///
    /// LIFO nesting is **not** supported (DR1-003): the design guarantees by
    /// construction that a freeze region never overlaps with another freeze
    /// region from the same handle, so a plain `AtomicBool` is sufficient. The
    /// guard's `Drop` always clears `freeze` regardless of nesting depth.
    pub fn freeze_for_inference(&self) -> FreezeGuard {
        self.freeze_inner()
    }

    /// Pause the footer worker for a rustyline prompt. Same implementation as
    /// `freeze_for_inference`; the two function names exist so call-sites
    /// document intent (DR1-003).
    pub fn freeze_for_prompt(&self) -> FreezeGuard {
        self.freeze_inner()
    }

    /// Shared body for `freeze_for_inference` / `freeze_for_prompt`. Disabled
    /// handles (no attached state) produce a guard whose `Drop` is a no-op.
    fn freeze_inner(&self) -> FreezeGuard {
        let Some(state) = &self.state else {
            return FreezeGuard { state: None };
        };
        state.freeze.store(true, Ordering::SeqCst);
        let (lock, cvar) = &state.wake;
        // Take the wake lock briefly so the worker — which holds it across
        // `wait_timeout` — is guaranteed to observe the freeze flag flip on
        // its next loop iteration. Recover from poisoning per fail-open.
        let _g = lock.lock().unwrap_or_else(|p| p.into_inner());
        cvar.notify_all();
        FreezeGuard {
            state: Some(state.clone()),
        }
    }

    /// Test-only constructor: build a handle wired to a freshly-allocated
    /// `FooterStateInner`. Lets Phase B unit tests assert that
    /// `publish_tokens` / `publish_flags` write through, without depending on
    /// a real `FooterLease` (which gates on TTY in cargo's harness).
    #[cfg(test)]
    pub(super) fn with_state(state: Arc<FooterStateInner>) -> Self {
        Self {
            enabled: false,
            state: Some(state),
        }
    }

    /// Test-only accessor for the inner state. Used to read back published
    /// values in unit tests.
    #[cfg(test)]
    pub(super) fn state(&self) -> Option<&Arc<FooterStateInner>> {
        self.state.as_ref()
    }
}

impl Drop for FreezeGuard {
    fn drop(&mut self) {
        // Phase D: clear `freeze` and notify the worker so the next render
        // tick fires immediately. No-op when the originating handle was
        // disabled (state == None). Recover from poison so a panicked
        // peer thread cannot leak a stuck freeze (fail-open per AC17).
        let Some(state) = self.state.take() else {
            return;
        };
        state.freeze.store(false, Ordering::SeqCst);
        let (lock, cvar) = &state.wake;
        let _g = lock.lock().unwrap_or_else(|p| p.into_inner());
        cvar.notify_all();
    }
}

/// Daemon thread render loop. Runs until `stop` is set, ticking every
/// `TICK` (200ms). On each tick:
///   1. Skip when `state.self_disabled` (Phase E will set this on persistent
///      write failures).
///   2. Skip when `state.freeze` (Phase D will set this around stdout-emitting
///      regions).
///   3. Re-read `terminal::size()`. Anomalous values flip self-disable.
///   4. Build the footer ANSI string and write through the shared writer.
///   5. Wait on the wake Condvar for at most `TICK`.
///
/// The loop is wrapped in `catch_unwind` by the spawn site so a panic inside
/// any of the steps cannot abort the process (matches spinner.rs ethos).
fn render_loop(
    state: Arc<FooterStateInner>,
    stop: Arc<AtomicBool>,
    writer: Arc<Mutex<FooterWriter>>,
) {
    let (lock, cvar) = &state.wake;
    // Cache the previous size so we only re-emit DECSTBM when the row count
    // changes (AC3). Initialise from a fresh probe so the first tick already
    // has a baseline.
    let mut prev_rows: u16 = match crossterm::terminal::size() {
        Ok((_, r)) => r,
        Err(_) => 0,
    };
    while !stop.load(Ordering::SeqCst) {
        if state.self_disabled.load(Ordering::SeqCst) {
            // Permanently disabled: park on the wake cvar without rendering.
            let g = lock.lock().unwrap_or_else(|p| p.into_inner());
            let _ = cvar.wait_timeout(g, TICK);
            continue;
        }
        let frozen = state.freeze.load(Ordering::SeqCst);
        if !frozen {
            // 3. terminal::size() — fall back to the rows captured at acquire
            // time when the live probe fails (CI / non-TTY harness writers).
            // Anomalous values still flip permanent self-disable below.
            let size = crossterm::terminal::size();
            let (cols, rows) = match size {
                Ok((c, r)) => (c, r),
                Err(_) => {
                    let fallback = state.fallback_rows.load(Ordering::Relaxed);
                    if fallback > 0 {
                        (80, fallback)
                    } else {
                        // No fallback recorded — wait and retry next tick;
                        // transient ioctl errors must not self-disable.
                        let g = lock.lock().unwrap_or_else(|p| p.into_inner());
                        let _ = cvar.wait_timeout(g, TICK);
                        continue;
                    }
                }
            };
            if rows < 2 || cols == 0 {
                // Anomalous size while running — self-disable to avoid
                // emitting garbage. (DR4-001)
                state.self_disabled.store(true, Ordering::SeqCst);
                continue;
            }

            // AC3: when the row count changed, re-emit DECSTBM so the scroll
            // region still leaves the bottom row free.
            if rows != prev_rows
                && let Some(decstbm) = ansi::build_decstbm(rows)
                && let Ok(mut w) = writer.lock()
            {
                let _ = w.write_all(decstbm.as_bytes());
            }
            prev_rows = rows;

            // 4. Render the footer line at row=`rows`, col=1.
            let snapshot = state.snapshot();
            let line = build_footer_line(&snapshot, /* use_color */ true);
            let mut out = String::with_capacity(line.len() + 32);
            out.push_str(ansi::save_cursor());
            out.push_str(&ansi::move_to(rows, 1));
            out.push_str(ansi::clear_line());
            out.push_str(&line);
            out.push_str(ansi::restore_cursor());

            let write_result = {
                let mut w = writer.lock().unwrap_or_else(|p| p.into_inner());
                let r = w.write_all(out.as_bytes());
                let _ = w.flush();
                r
            };
            if write_result.is_err() {
                // AC17: write failure → permanent self-disable. Phase E
                // refines the warning text and metrics.
                state.self_disabled.store(true, Ordering::SeqCst);
                tracing::warn!("anvil footer: write failed; self-disabling");
                continue;
            }
        }
        // 5. Wait up to TICK for stop / freeze change / publish_* wake.
        let g = lock.lock().unwrap_or_else(|p| p.into_inner());
        let _ = cvar.wait_timeout(g, TICK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;

    /// Process-wide serialiser for tests that touch `FOOTER_LEASE_HELD` /
    /// global panic hook state. We avoid `serial_test` (would add a new
    /// dependency, forbidden by the work plan); a plain `Mutex<()>` does the
    /// job because `cargo test`'s default thread pool only contends inside
    /// the same binary.
    static PHASE_C_TEST_LOCK: StdMutex<()> = StdMutex::new(());

    /// RAII helper: take the phase-C lock and on drop ensure both the lease
    /// bit and the FOOTER_LEASE_HELD mutex are in clean state for the next
    /// test, even when the body panics.
    struct PhaseCTestGuard {
        _g: std::sync::MutexGuard<'static, ()>,
    }

    impl PhaseCTestGuard {
        fn acquire() -> Self {
            // Recover from a poisoned guard so a previous test panic does not
            // permanently break Phase C tests.
            let g = match PHASE_C_TEST_LOCK.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            // Defensive cleanup: clear stale state from a previous panicked
            // test. Use the recovery helper because a previous Phase C test
            // may have intentionally poisoned the lease mutex.
            let mut h = lock_lease_bit_or_recover();
            *h = false;
            Self { _g: g }
        }
    }

    impl Drop for PhaseCTestGuard {
        fn drop(&mut self) {
            // Always clear the bit on test exit (the Drop of FooterLease
            // should already have done so on the happy path; this is purely
            // defensive for tests that exit early). Recover from poison.
            let mut h = lock_lease_bit_or_recover();
            *h = false;
        }
    }

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
        let _g = PhaseCTestGuard::acquire();
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
        let _g = PhaseCTestGuard::acquire();
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

    /// AC15 simulation via direct lock manipulation (no TTY required).
    #[test]
    fn lease_bit_is_set_and_cleared_under_simulated_acquire() {
        let _g = PhaseCTestGuard::acquire();
        // PhaseCTestGuard already cleared the bit (with poison recovery).
        // Simulate a successful acquire by manually flipping the bit.
        {
            let mut g = lock_lease_bit_or_recover();
            assert!(!*g, "lease bit must start cleared");
            *g = true;
        }
        let lease = FooterLease {
            holds_lease_bit: true,
            inner: None,
            handle: FooterHandle {
                enabled: false,
                state: None,
            },
        };
        assert!(*lock_lease_bit_or_recover(), "bit must be set while held");
        drop(lease);
        assert!(
            !*lock_lease_bit_or_recover(),
            "Drop must release the lease bit"
        );
    }

    /// AC15: with the lease bit already set, `acquire` returns a no-op lease
    /// that does NOT release the bit on its own Drop.
    #[test]
    fn second_acquire_under_held_bit_returns_disabled() {
        let _g = PhaseCTestGuard::acquire();
        // Pre-set the bit as if a real lease were already alive.
        {
            let mut g = lock_lease_bit_or_recover();
            *g = true;
        }
        let cfg = config_with_footer(true);
        let lease = FooterLease::acquire(&cfg);
        assert!(!lease.handle_clone().is_enabled());
        drop(lease);
        assert!(
            *lock_lease_bit_or_recover(),
            "no-op lease must not release the lease bit on Drop"
        );
        // Cleanup happens via PhaseCTestGuard::Drop.
    }

    // ===== Phase B: ANSI builders (Task B.1) ===============================

    #[test]
    fn build_decstbm_reserves_last_row_for_footer() {
        assert_eq!(ansi::build_decstbm(24), Some("\x1b[1;23r".to_string()));
    }

    #[test]
    fn build_decstbm_handles_small_and_large_rows() {
        assert_eq!(ansi::build_decstbm(2), Some("\x1b[1;1r".to_string()));
        assert_eq!(
            ansi::build_decstbm(u16::MAX),
            Some(format!("\x1b[1;{}r", u16::MAX - 1))
        );
    }

    #[test]
    fn build_decstbm_disables_on_anomalous_size() {
        assert_eq!(ansi::build_decstbm(0), None);
        assert_eq!(ansi::build_decstbm(1), None);
    }

    #[test]
    fn build_decstbm_reset_is_fixed_string() {
        assert_eq!(ansi::build_decstbm_reset(), "\x1b[r");
    }

    #[test]
    fn move_to_is_cup_with_row_and_col() {
        assert_eq!(ansi::move_to(24, 1), "\x1b[24;1H");
        assert_eq!(ansi::move_to(1, 80), "\x1b[1;80H");
    }

    #[test]
    fn save_and_restore_cursor_are_decsc_decrc() {
        assert_eq!(ansi::save_cursor(), "\x1b[s");
        assert_eq!(ansi::restore_cursor(), "\x1b[u");
    }

    #[test]
    fn clear_line_matches_spinner_pattern() {
        assert_eq!(ansi::clear_line(), "\r\x1b[2K");
    }

    // ===== Phase B: bar_for_ratio (Task B.2) ================================

    #[test]
    fn bar_for_ratio_zero_is_all_empty_cells() {
        assert_eq!(bar_for_ratio(0.0, /* use_color */ false), "░░░░░░░░ 0%");
    }

    #[test]
    fn bar_for_ratio_quarter_fills_two_cells() {
        assert_eq!(bar_for_ratio(0.25, false), "██░░░░░░ 25%");
    }

    #[test]
    fn bar_for_ratio_forty_two_percent_matches_issue_body() {
        assert_eq!(bar_for_ratio(0.42, false), "███░░░░░ 42%");
    }

    #[test]
    fn bar_for_ratio_ninety_percent_is_yellow_when_color_enabled() {
        let with_color = bar_for_ratio(0.9, true);
        assert!(
            with_color.starts_with("\x1b[33m") && with_color.ends_with("\x1b[0m"),
            "90% must wrap in yellow ANSI when color enabled, got {with_color:?}"
        );
        assert!(with_color.contains("███████░ 90%"));
    }

    #[test]
    fn bar_for_ratio_ninety_percent_no_color_omits_ansi() {
        assert_eq!(bar_for_ratio(0.9, false), "███████░ 90%");
    }

    #[test]
    fn bar_for_ratio_exactly_full_is_red_hundred_percent() {
        let with_color = bar_for_ratio(1.0, true);
        assert!(
            with_color.starts_with("\x1b[31m") && with_color.ends_with("\x1b[0m"),
            "100% must wrap in red when color enabled, got {with_color:?}"
        );
        assert!(with_color.contains("████████ 100%"));
        assert_eq!(bar_for_ratio(1.0, false), "████████ 100%");
    }

    #[test]
    fn bar_for_ratio_over_full_shows_greater_than_hundred() {
        let with_color = bar_for_ratio(1.5, true);
        assert!(with_color.starts_with("\x1b[31m"));
        assert!(with_color.contains("████████ >100%"));
        assert_eq!(bar_for_ratio(1.5, false), "████████ >100%");
    }

    #[test]
    fn bar_for_ratio_nan_and_negative_clamp_to_zero() {
        assert_eq!(bar_for_ratio(f64::NAN, false), "░░░░░░░░ 0%");
        assert_eq!(bar_for_ratio(-0.5, false), "░░░░░░░░ 0%");
    }

    // ===== Phase B: token formatter ========================================

    #[test]
    fn format_token_count_tiers() {
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(42), "42");
        assert_eq!(format_token_count(999), "999");
        assert_eq!(format_token_count(1_000), "1k");
        assert_eq!(format_token_count(1_234), "1.2k");
        assert_eq!(format_token_count(9_999), "9.9k");
        assert_eq!(format_token_count(10_100), "10.1k");
        assert_eq!(format_token_count(24_000), "24k");
        assert_eq!(format_token_count(100_000), "100k");
    }

    // ===== Phase B: build_footer_line (Task B.3) ===========================

    fn snap(
        tokens: usize,
        budget: usize,
        mode: ExecutionMode,
        log: LogLevel,
        yes: bool,
    ) -> FooterSnapshot {
        let ratio = if budget == 0 {
            0.0
        } else {
            tokens as f64 / budget as f64
        };
        FooterSnapshot {
            tokens,
            budget,
            mode,
            log,
            yes,
            ratio,
        }
    }

    /// DR2-004 MANDATORY: Exact-match to the Issue #430 body example line.
    #[test]
    fn build_footer_line_matches_issue_body_example() {
        let s = snap(10_100, 24_000, ExecutionMode::Plan, LogLevel::Verbose, true);
        assert_eq!(
            build_footer_line(&s, /* use_color */ false),
            "[plan]  ███░░░░░ 42% (10.1k / 24k)  [verbose]  [yes]"
        );
    }

    #[test]
    fn build_footer_line_act_mode_info_log_no_yes_is_minimal() {
        let s = snap(0, 24_000, ExecutionMode::Act, LogLevel::Info, false);
        assert_eq!(build_footer_line(&s, false), "[act]  ░░░░░░░░ 0% (0 / 24k)");
    }

    #[test]
    fn build_footer_line_trace_log_level_shown_as_trace() {
        let s = snap(500, 24_000, ExecutionMode::Act, LogLevel::Trace, false);
        assert!(build_footer_line(&s, false).contains("  [trace]"));
        assert!(!build_footer_line(&s, false).contains("[verbose]"));
    }

    #[test]
    fn build_footer_line_verbose_hides_trace() {
        let s = snap(500, 24_000, ExecutionMode::Plan, LogLevel::Verbose, false);
        let line = build_footer_line(&s, false);
        assert!(line.contains("  [verbose]"));
        assert!(!line.contains("[trace]"));
    }

    #[test]
    fn build_footer_line_yes_flag_only_when_true() {
        let s_on = snap(0, 24_000, ExecutionMode::Act, LogLevel::Info, true);
        let s_off = snap(0, 24_000, ExecutionMode::Act, LogLevel::Info, false);
        assert!(build_footer_line(&s_on, false).ends_with("  [yes]"));
        assert!(!build_footer_line(&s_off, false).contains("[yes]"));
    }

    #[test]
    fn build_footer_line_separator_is_two_spaces() {
        let s = snap(10_100, 24_000, ExecutionMode::Plan, LogLevel::Verbose, true);
        let line = build_footer_line(&s, false);
        assert!(line.contains("]  "), "expected 2-space separators: {line}");
        assert!(
            !line.contains("]   "),
            "must not have 3-space separators: {line}"
        );
    }

    // ===== Phase B: FooterStateInner publish API (Task B.4) ================

    #[test]
    fn publish_tokens_writes_through_state() {
        let state = FooterStateInner::new(24_000, FooterFlags::default());
        let h = FooterHandle::with_state(state.clone());
        h.publish_tokens(12_345);
        assert_eq!(state.tokens.load(Ordering::Relaxed), 12_345);
    }

    #[test]
    fn publish_flags_writes_through_state() {
        let state = FooterStateInner::new(24_000, FooterFlags::default());
        let h = FooterHandle::with_state(state.clone());
        h.publish_flags(ExecutionMode::Plan, LogLevel::Trace, true);
        let flags = *state.flags.lock().unwrap();
        assert_eq!(flags.mode, ExecutionMode::Plan);
        assert_eq!(flags.log, LogLevel::Trace);
        assert!(flags.yes);
    }

    #[test]
    fn publish_without_state_is_noop() {
        let h = FooterHandle::disabled();
        h.publish_tokens(999);
        h.publish_flags(ExecutionMode::Plan, LogLevel::Trace, true);
        assert!(h.state().is_none());
    }

    #[test]
    fn snapshot_reads_back_published_values() {
        let state = FooterStateInner::new(24_000, FooterFlags::default());
        let h = FooterHandle::with_state(state.clone());
        h.publish_tokens(10_100);
        h.publish_flags(ExecutionMode::Plan, LogLevel::Verbose, true);

        let s = state.snapshot();
        assert_eq!(s.tokens, 10_100);
        assert_eq!(s.budget, 24_000);
        assert_eq!(s.mode, ExecutionMode::Plan);
        assert_eq!(s.log, LogLevel::Verbose);
        assert!(s.yes);
        assert!((s.ratio - (10_100.0 / 24_000.0)).abs() < 1e-9);
    }

    #[test]
    fn snapshot_zero_budget_yields_zero_ratio() {
        let state = FooterStateInner::new(0, FooterFlags::default());
        let s = state.snapshot();
        assert_eq!(s.ratio, 0.0);
    }

    // ===== Phase C: install/Drop round-trip (Task C.1, C.3, C.4) ===========

    /// In-memory writer that captures everything the lease/worker emits.
    struct CaptureWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl CaptureWriter {
        fn new(buf: Arc<Mutex<Vec<u8>>>) -> Self {
            Self { buf }
        }
    }

    impl Write for CaptureWriter {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            let mut g = self.buf.lock().unwrap_or_else(|p| p.into_inner());
            g.extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Task C.4: `acquire_for_test` writes DECSTBM on install and DECSTBM
    /// reset + cursor home on Drop. Captures the full byte stream and
    /// asserts on prefix / suffix.
    #[test]
    fn acquire_for_test_writes_decstbm_and_drop_resets() {
        let _g = PhaseCTestGuard::acquire();
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let writer: FooterWriter = Box::new(CaptureWriter::new(buf.clone()));
        let lease = FooterLease::acquire_for_test(24, writer);
        assert!(
            lease.handle_clone().is_enabled(),
            "acquire_for_test on rows=24 must produce an enabled lease"
        );
        // Brief pause so the worker has a chance to write at least once,
        // but we do not depend on it for the DECSTBM-set assertion.
        std::thread::sleep(Duration::from_millis(50));
        drop(lease);

        let captured = buf.lock().unwrap();
        let s = String::from_utf8_lossy(&captured);
        // DECSTBM set should appear early in the stream.
        assert!(
            s.contains("\x1b[1;23r"),
            "expected DECSTBM set (rows=24 → top=23) in captured output: {s:?}"
        );
        // DECSTBM reset must appear on Drop.
        assert!(
            s.contains("\x1b[r"),
            "expected DECSTBM reset on Drop: {s:?}"
        );
        // Cursor home to row 24, col 1 (move_to(rows, 1)) must appear on Drop.
        assert!(
            s.contains("\x1b[24;1H"),
            "expected cursor home on Drop: {s:?}"
        );
    }

    /// Task C.1: 1st enabled, 2nd disabled while 1st alive, 3rd enabled
    /// after 1st drop.
    #[test]
    fn lease_round_trip_enables_disables_enables() {
        let _g = PhaseCTestGuard::acquire();
        let buf1 = Arc::new(Mutex::new(Vec::<u8>::new()));
        let lease1 = FooterLease::acquire_for_test(24, Box::new(CaptureWriter::new(buf1.clone())));
        assert!(
            lease1.handle_clone().is_enabled(),
            "first lease must be enabled"
        );

        let buf2 = Arc::new(Mutex::new(Vec::<u8>::new()));
        let lease2 = FooterLease::acquire_for_test(24, Box::new(CaptureWriter::new(buf2.clone())));
        assert!(
            !lease2.handle_clone().is_enabled(),
            "second concurrent lease must be disabled (AC15)"
        );
        drop(lease2);
        // Lease bit must still be held by lease1.
        assert!(*lock_lease_bit_or_recover(), "bit still held by lease1");

        drop(lease1);
        assert!(
            !*lock_lease_bit_or_recover(),
            "Drop of lease1 must clear the bit"
        );

        let buf3 = Arc::new(Mutex::new(Vec::<u8>::new()));
        let lease3 = FooterLease::acquire_for_test(24, Box::new(CaptureWriter::new(buf3.clone())));
        assert!(
            lease3.handle_clone().is_enabled(),
            "third lease (after drop) must be enabled"
        );
        drop(lease3);
    }

    /// Task C.1: poisoned lease mutex still allows recovery via `into_inner`.
    /// Force a panic inside a thread holding the lease mutex, then assert
    /// the next lock attempt recovers cleanly.
    #[test]
    fn lease_lock_recovers_from_poisoning() {
        let _g = PhaseCTestGuard::acquire();
        // Reset bit (PhaseCTestGuard already did this with recovery).
        // Spawn a thread that locks the mutex and panics.
        let join = std::thread::spawn(|| {
            let _guard = lease_lock().lock().unwrap();
            panic!("intentional poison");
        });
        // Joining returns Err because the thread panicked, which is fine.
        let _ = join.join();
        // Now try to acquire: lock_lease_bit_or_recover must not panic.
        let mut h = lock_lease_bit_or_recover();
        assert!(!*h, "bit value preserved across poisoning");
        *h = true;
        drop(h);
        // Cleanup via recovery helper (raw lock() would silently fail because
        // the mutex is now permanently poisoned).
        let mut h = lock_lease_bit_or_recover();
        *h = false;
    }

    /// Task C.3: render_loop terminates on stop signal (smoke test).
    /// Spawns the loop directly with an in-memory writer, asserts it exits
    /// after stop is set + cvar notified.
    #[test]
    fn render_loop_terminates_on_stop_signal() {
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let writer: Arc<Mutex<FooterWriter>> =
            Arc::new(Mutex::new(Box::new(CaptureWriter::new(buf.clone()))));
        let state = FooterStateInner::new(24_000, FooterFlags::default());
        let stop = Arc::new(AtomicBool::new(false));

        let state_t = state.clone();
        let stop_t = stop.clone();
        let writer_t = writer.clone();
        let handle = thread::spawn(move || {
            render_loop(state_t, stop_t, writer_t);
        });
        // Let it tick at least once.
        std::thread::sleep(Duration::from_millis(50));
        stop.store(true, Ordering::SeqCst);
        let (lock, cvar) = &state.wake;
        {
            let _g = lock.lock().unwrap();
            cvar.notify_all();
        }
        // Must exit promptly (well under TICK + slop).
        let join_start = std::time::Instant::now();
        handle.join().expect("render_loop must terminate cleanly");
        assert!(
            join_start.elapsed() < Duration::from_secs(2),
            "render_loop must exit within 2s of stop signal"
        );
    }

    /// Task C.3: render_loop honours self_disabled and writes nothing while it
    /// is set.
    #[test]
    fn render_loop_skips_writes_when_self_disabled() {
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let writer: Arc<Mutex<FooterWriter>> =
            Arc::new(Mutex::new(Box::new(CaptureWriter::new(buf.clone()))));
        let state = FooterStateInner::new(24_000, FooterFlags::default());
        // Pre-set self_disabled before the worker starts.
        state.self_disabled.store(true, Ordering::SeqCst);
        let stop = Arc::new(AtomicBool::new(false));

        let state_t = state.clone();
        let stop_t = stop.clone();
        let writer_t = writer.clone();
        let handle = thread::spawn(move || {
            render_loop(state_t, stop_t, writer_t);
        });
        std::thread::sleep(Duration::from_millis(80));
        stop.store(true, Ordering::SeqCst);
        let (lock, cvar) = &state.wake;
        {
            let _g = lock.lock().unwrap();
            cvar.notify_all();
        }
        handle.join().expect("render_loop must terminate");
        let captured = buf.lock().unwrap();
        assert!(
            captured.is_empty(),
            "self_disabled worker must not write; got {captured:?}"
        );
    }

    /// Task C.2: panic hook chain — the prev hook is invoked for in-process
    /// panics. We install our own prev hook before `acquire_for_test`, then
    /// trigger a panic via `catch_unwind` and assert the prev hook ran.
    #[test]
    fn panic_hook_chain_invokes_prev() {
        let _g = PhaseCTestGuard::acquire();
        // Install a prev hook that bumps a counter via a static AtomicUsize.
        static PREV_HOOK_HITS: AtomicUsize = AtomicUsize::new(0);
        PREV_HOOK_HITS.store(0, Ordering::SeqCst);
        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_info| {
            PREV_HOOK_HITS.fetch_add(1, Ordering::SeqCst);
        }));

        // Acquire chains over our prev hook.
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let lease = FooterLease::acquire_for_test(24, Box::new(CaptureWriter::new(buf.clone())));
        assert!(lease.handle_clone().is_enabled());

        // Trigger a panic in a thread (so the test harness does not abort);
        // the panic hook chain (ours → prev) fires, bumping PREV_HOOK_HITS.
        let _ = std::panic::catch_unwind(|| {
            panic!("hook-chain test panic");
        });
        assert!(
            PREV_HOOK_HITS.load(Ordering::SeqCst) >= 1,
            "prev panic hook must have been called via the chain"
        );

        // Drop the lease (restores original prev hook installed before our
        // acquire). Then reset to the truly original hook.
        drop(lease);
        // Restore the test runner's original hook to avoid affecting later
        // tests' panic output.
        let _restored_test_hook = std::panic::take_hook();
        std::panic::set_hook(original);
    }

    // ===== Phase D: FreezeGuard rendezvous (Task D.1) ======================

    /// Task D.1 (Red→Green): `freeze_for_inference` flips `state.freeze`
    /// so the worker stops re-rendering. The flag stays set as long as the
    /// returned guard is alive, even across publish_tokens calls.
    #[test]
    fn freeze_guard_blocks_writes_until_drop() {
        let _g = PhaseCTestGuard::acquire();
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let lease = FooterLease::acquire_for_test(24, Box::new(CaptureWriter::new(buf.clone())));
        assert!(lease.handle_clone().is_enabled());
        let handle = lease.handle_clone();
        let state = handle
            .state()
            .expect("enabled handle must carry state")
            .clone();

        // Take freeze guard. Worker must stop re-rendering.
        let guard = handle.freeze_for_inference();
        assert!(
            state.freeze.load(Ordering::SeqCst),
            "freeze flag must be set while guard is alive"
        );

        // Snapshot current capture length, wait > 1 tick, assert no growth
        // attributable to a frozen worker writing footer ANSI. The capture
        // may already contain DECSTBM + at most one race-window render from
        // before freeze landed; after that nothing new should appear.
        let len_after_freeze = buf.lock().unwrap().len();
        std::thread::sleep(Duration::from_millis(TICK.as_millis() as u64 * 2 + 50));
        let len_after_wait = buf.lock().unwrap().len();
        assert_eq!(
            len_after_wait, len_after_freeze,
            "no further footer writes should occur while freeze guard is alive"
        );

        drop(guard);
        // Guard drop clears freeze.
        assert!(
            !state.freeze.load(Ordering::SeqCst),
            "freeze flag must be cleared once guard is dropped"
        );
        drop(lease);
    }

    /// Task D.1 (Red→Green): dropping the guard wakes the worker so the
    /// next render fires within the tick window without further publishes.
    #[test]
    fn freeze_guard_drop_resumes_render() {
        let _g = PhaseCTestGuard::acquire();
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let lease = FooterLease::acquire_for_test(24, Box::new(CaptureWriter::new(buf.clone())));
        assert!(lease.handle_clone().is_enabled());
        let handle = lease.handle_clone();
        let state = handle
            .state()
            .expect("enabled handle must carry state")
            .clone();

        // Freeze, then drop, then assert the worker resumed by observing
        // a footer line write within ~2 ticks. Drop publishes a fresh token
        // value first so the resumed render produces fresh bytes (not an
        // exact duplicate of any pre-freeze snapshot).
        let guard = handle.freeze_for_inference();
        assert!(state.freeze.load(Ordering::SeqCst));
        drop(guard);
        // Publish a distinctive value the worker will format into the line.
        handle.publish_tokens(7_777);

        // Wake-on-drop should be observed within one TICK. Allow generous
        // slack for slow CI.
        let deadline = std::time::Instant::now() + Duration::from_millis(4000);
        let mut saw_token = false;
        while std::time::Instant::now() < deadline {
            let captured = buf.lock().unwrap().clone();
            let s = String::from_utf8_lossy(&captured);
            // 7777 → "7.7k" by `format_token_count`.
            if s.contains("7.7k") {
                saw_token = true;
                break;
            }
            drop(captured);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            saw_token,
            "worker must resume rendering after FreezeGuard drop"
        );
        drop(lease);
    }
}
