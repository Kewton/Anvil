//! Phase 0 tests for the render-event stream scaffolding (Issue #379).
//!
//! Covers:
//! - `RenderCoordinator::start` / `Drop` lifecycle (thread spawn + join).
//! - `RenderSender::try_send_coalesce` basic success path on a bounded channel.
//! - Non-interactive (stub) mode: events are accepted and drained without
//!   touching stderr, and the coordinator still shuts down cleanly on drop.
//! - `ConsoleRenderContext` backwards-compatible deserialization when the new
//!   `ansi_enabled` field is absent from old JSON payloads (DR2-002).

use anvil::contracts::{AppStateSnapshot, ConsoleRenderContext, RuntimeState};
use anvil::tui::{RenderCoordinator, RenderEvent};

#[test]
fn render_coordinator_starts_and_drops_cleanly_in_stub_mode() {
    // stderr_enabled=false -> stub render loop, safe for TTY-less CI.
    let coordinator = RenderCoordinator::start(false);
    let sender = coordinator.sender();

    // Pushing a few events must not panic and must not block.
    sender
        .try_send_coalesce(RenderEvent::TokenDelta("hello".to_string()))
        .expect("token delta should enqueue");
    sender
        .try_send_coalesce(RenderEvent::SpinnerTick)
        .expect("spinner tick should enqueue");
    sender
        .try_send_coalesce(RenderEvent::SpinnerStop)
        .expect("spinner stop should enqueue");
    sender
        .try_send_coalesce(RenderEvent::ThinkingStart {
            model: "local-test".to_string(),
        })
        .expect("thinking start should enqueue");
    sender
        .try_send_coalesce(RenderEvent::ThinkingEnd)
        .expect("thinking end should enqueue");

    // Callers MUST drop every cloned sender before the coordinator so the
    // render thread's `recv()` returns `Disconnected` and `Drop` can join.
    drop(sender);
    drop(coordinator);
}

#[test]
fn render_sender_is_clone_and_send() {
    let coordinator = RenderCoordinator::start(false);
    let sender = coordinator.sender();
    let sender_clone = sender.clone();

    let handle = std::thread::spawn(move || {
        sender_clone
            .try_send_coalesce(RenderEvent::TokenDelta("from-thread".to_string()))
            .expect("cross-thread enqueue should succeed");
    });
    handle.join().expect("worker thread should join");

    sender
        .try_send_coalesce(RenderEvent::TokenDelta("from-main".to_string()))
        .expect("main thread enqueue should succeed");

    // Drop our local sender before dropping the coordinator (see
    // RenderCoordinator doc comment — all senders must be gone first).
    drop(sender);
    drop(coordinator);
}

#[test]
fn render_coordinator_drain_and_flush_is_noop_when_empty() {
    let coordinator = RenderCoordinator::start(false);
    // Must not hang, must not panic on empty channel.
    coordinator.drain_and_flush();
    drop(coordinator);
}

#[test]
fn console_render_context_deserializes_without_ansi_enabled_field() {
    // Old session JSON (before Issue #379) will not contain `ansi_enabled`.
    // `#[serde(default)]` must keep it deserializable.
    let old_json = serde_json::json!({
        "snapshot": AppStateSnapshot::new(RuntimeState::Ready),
        "model_name": "legacy-model",
        "messages": [],
        "history_summary": null,
    });

    let ctx: ConsoleRenderContext =
        serde_json::from_value(old_json).expect("legacy JSON should deserialize");
    assert_eq!(ctx.model_name, "legacy-model");
    assert!(
        !ctx.ansi_enabled,
        "ansi_enabled must default to false for legacy payloads"
    );
}

#[test]
fn console_render_context_serializes_ansi_enabled_field() {
    // New payloads round-trip cleanly.
    let ctx = ConsoleRenderContext {
        snapshot: AppStateSnapshot::new(RuntimeState::Ready),
        model_name: "m".to_string(),
        messages: vec![],
        history_summary: None,
        ansi_enabled: true,
    };
    let encoded = serde_json::to_string(&ctx).expect("serialize");
    assert!(encoded.contains("\"ansi_enabled\":true"));

    let decoded: ConsoleRenderContext =
        serde_json::from_str(&encoded).expect("round-trip deserialize");
    assert!(decoded.ansi_enabled);
}
