//! Render-event streaming scaffolding (Issue #379 Phase 0).
//!
//! This module introduces the `mpsc::sync_channel` based coordination
//! primitives between the turn-executing thread(s) and the single
//! stderr-owning render thread. At this phase the render thread is
//! intentionally minimal — Phase 3 will extend it with real `TokenDelta`,
//! Spinner and StatusBar rendering while the shape of the public API stays
//! stable.
//!
//! Design references:
//! - `dev-reports/design/issue-379-tui-improvements-design-policy.md` D3 / DR1-004 / DR4-001
//! - `dev-reports/issue/379/work-plan.md` Task 0.1 / Task 0.2

use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};

use crate::tooling::progress::ToolProgressEntry;

/// Capacity of the bounded render-event queue.
///
/// DR4-001 requires a bounded channel so that a runaway producer cannot
/// exhaust memory. 1024 matches the design-policy table.
const RENDER_CHANNEL_CAPACITY: usize = 1024;

/// Minimal status-line payload (Phase 5 will extend it).
///
/// Mirrors the footer segments produced by [`crate::tui::Tui::render_console`]
/// so that the render thread can update the bottom status bar without having
/// to re-run the full console renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    /// Human-readable lifecycle state (e.g. `"Thinking"`, `"Working"`).
    pub state: String,
    /// Formatted elapsed time (e.g. `"12s"`, `"-"`).
    pub elapsed: String,
    /// Model name shown in the `model:` footer segment.
    pub model: String,
    /// Context-window usage percentage (0-100).
    pub ctx_pct: u8,
    /// Optional inference performance string (e.g. `"42 tok/s"`).
    pub perf: Option<String>,
    /// Optional active plan-item marker (e.g. `"3/5"`).
    pub active: Option<String>,
    /// Optional last event label.
    pub event: Option<String>,
}

/// Events sent from any producer thread to the render thread.
///
/// DR1-004: No `Shutdown` variant — the coordinator drops `tx`, which causes
/// `rx.recv()` to return `Err(Disconnected)` and the render thread exits.
#[derive(Debug, Clone)]
pub enum RenderEvent {
    /// Incremental assistant token to be streamed to stderr.
    TokenDelta(String),
    /// Advance the spinner one frame.
    SpinnerTick,
    /// Stop and clear the spinner.
    SpinnerStop,
    /// Update an in-flight tool's progress display.
    ToolProgress(ToolProgressEntry),
    /// Replace the current footer / status bar.
    StatusUpdate(StatusLine),
    /// Begin a "thinking" phase (drives the reasoning indicator).
    ThinkingStart {
        /// Model name to display alongside the indicator.
        model: String,
    },
    /// End a "thinking" phase.
    ThinkingEnd,
}

/// Cloneable handle used by producers to push render events.
///
/// Wraps `SyncSender` so the underlying channel can later be replaced
/// (e.g. with a batched variant) without touching call sites.
#[derive(Clone)]
pub struct RenderSender {
    tx: SyncSender<RenderEvent>,
}

impl RenderSender {
    /// Attempt to send an event without blocking.
    ///
    /// The public API is named `try_send_coalesce` because Phase 3 will add
    /// coalesce-on-full logic for `TokenDelta`. In Phase 0 we simply forward
    /// to `try_send` and return the error verbatim so producers can decide
    /// whether to retry, drop, or merge.
    ///
    /// The `Err` variant is intentionally sized to hold the un-sent
    /// `RenderEvent` payload so coalesce can recover it — this is the same
    /// shape as `SyncSender::try_send` itself.
    #[allow(clippy::result_large_err)]
    pub fn try_send_coalesce(&self, event: RenderEvent) -> Result<(), TrySendError<RenderEvent>> {
        self.tx.try_send(event)
    }

    /// Blocking send — used only from sites where a token MUST NOT be
    /// dropped (e.g. final flush before stdout frame output).
    ///
    /// See [`Self::try_send_coalesce`] for the rationale behind the
    /// large `Err` variant lint allow.
    #[allow(clippy::result_large_err)]
    pub fn send(&self, event: RenderEvent) -> Result<(), mpsc::SendError<RenderEvent>> {
        self.tx.send(event)
    }
}

/// Owns the render thread and the `SyncSender` half of the event channel.
///
/// Drop semantics (DR1-004):
/// 1. Drop the held `SyncSender` (by taking it out of the struct).
/// 2. The render thread observes `Err(RecvError)` on its `Receiver` and
///    exits its loop naturally.
/// 3. Join the thread handle so `run_live_turn` only returns once all
///    buffered output has been written to stderr (DR2-006).
///
/// **Invariant**: callers must drop every cloned `RenderSender` obtained
/// via [`Self::sender`] before letting the coordinator drop, otherwise the
/// render thread will never observe channel disconnection and the final
/// `join()` will block. This matches the production control flow where
/// `run_live_turn` owns both the coordinator and all producer senders.
pub struct RenderCoordinator {
    tx: Option<SyncSender<RenderEvent>>,
    handle: Option<JoinHandle<()>>,
    // Kept for future use (Phase 3+): propagate stderr-writing capability
    // to the render thread and expose it to `drain_and_flush`.
    stderr_enabled: bool,
}

impl RenderCoordinator {
    /// Start the render thread and return an owning handle.
    ///
    /// `stderr_enabled=false` puts the render loop into stub mode: events
    /// are drained but nothing is written to stderr. This is the path taken
    /// by non-interactive runs (`--exec`/`--oneshot`) and by unit tests on
    /// TTY-less CI.
    pub fn start(stderr_enabled: bool) -> Self {
        let (tx, rx) = mpsc::sync_channel::<RenderEvent>(RENDER_CHANNEL_CAPACITY);
        let handle = thread::Builder::new()
            .name("anvil-render".to_string())
            .spawn(move || render_loop(rx, stderr_enabled))
            .expect("spawn render thread");
        Self {
            tx: Some(tx),
            handle: Some(handle),
            stderr_enabled,
        }
    }

    /// Produce a cloneable sender for producer threads.
    pub fn sender(&self) -> RenderSender {
        let tx = self
            .tx
            .as_ref()
            .expect("sender accessed after coordinator drop started")
            .clone();
        RenderSender { tx }
    }

    /// Force the render thread to drain any buffered output before the
    /// caller writes a stdout frame (DR2-006).
    ///
    /// Phase 0 keeps this a best-effort no-op: the render loop does not yet
    /// buffer output. Phase 3 will wire a round-trip barrier (e.g. an
    /// oneshot ack channel) here. We still expose the API now so all call
    /// sites can be stabilised early.
    pub fn drain_and_flush(&self) {
        // Intentionally empty in Phase 0 — see doc comment.
        let _ = self.stderr_enabled;
    }
}

impl Drop for RenderCoordinator {
    fn drop(&mut self) {
        // Step 1: drop the sender so the render thread's `recv()` fails.
        self.tx.take();
        // Step 2: join the render thread so no output is lost.
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The background render loop.
///
/// Phase 0 is deliberately minimal: it drains all events and discards
/// them. Phase 3 will replace each arm with the real rendering logic while
/// keeping the `RenderEvent` enum shape stable.
fn render_loop(rx: Receiver<RenderEvent>, _stderr_enabled: bool) {
    // Even in stub mode we iterate over the receiver so that `try_send`
    // does not wedge on a full queue: events are consumed as fast as
    // producers push them.
    while let Ok(event) = rx.recv() {
        match event {
            RenderEvent::TokenDelta(_)
            | RenderEvent::SpinnerTick
            | RenderEvent::SpinnerStop
            | RenderEvent::ToolProgress(_)
            | RenderEvent::StatusUpdate(_)
            | RenderEvent::ThinkingStart { .. }
            | RenderEvent::ThinkingEnd => {
                // Phase 3 will expand each arm. Intentionally a no-op here.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinator_spawns_and_joins_in_stub_mode() {
        let coord = RenderCoordinator::start(false);
        let sender = coord.sender();
        sender
            .try_send_coalesce(RenderEvent::SpinnerTick)
            .expect("enqueue tick");
        // All senders must be dropped before the coordinator so the render
        // thread's `recv()` returns `Disconnected` and `Drop` can join.
        drop(sender);
        drop(coord); // must not hang
    }

    #[test]
    fn drain_and_flush_is_noop_when_empty() {
        let coord = RenderCoordinator::start(false);
        coord.drain_and_flush();
    }
}
