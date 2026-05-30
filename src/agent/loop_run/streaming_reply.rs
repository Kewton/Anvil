//! Issue #685 (parent #680, Phase 5): streaming reply render state +
//! chunk-handling flow extracted from `turn.rs`.
//!
//! Hosts:
//!
//! * `StreamingReplyRenderState` — bounded `(first_chunk, renderer)`
//!   pair built once per `chat_streaming_with_mode` call and threaded
//!   through the chunk callback. The renderer is `None` when markdown
//!   is fully disabled; otherwise it is a fresh
//!   `tui::markdown::MarkdownRenderer` configured for the current TTY
//!   colour / utf-8 capabilities.
//! * `streaming_reply_needs_prefix` / `streaming_reply_needs_trailing_newline`
//!   — pure predicates for the `"assistant> "` prefix and the final
//!   newline.
//! * `handle_streaming_assistant_chunk` / `finish_streaming_assistant_reply`
//!   — free fns (previously `impl Agent` associated fns) that operate
//!   on the render state only; no `&Agent` needed.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).
//! `turn.rs` is the only in-crate consumer.

use super::interrupt::InterruptFlag;
use super::spinner::SpinnerStopSignal;
use super::turn::USER_INTERRUPT_ERROR;
use super::turn_helpers::write_stdout_rendered;

pub(super) struct StreamingReplyRenderState {
    pub(super) first_chunk: bool,
    pub(super) renderer: Option<crate::tui::markdown::MarkdownRenderer>,
}

impl StreamingReplyRenderState {
    pub(super) fn new() -> Self {
        let markdown_disabled = crate::tui::markdown::markdown_fully_disabled();
        let color = crate::tui::markdown::color_enabled_for_markdown();
        let utf8 = crate::tui::markdown::markdown_unicode_enabled();
        tracing::debug!(
            disabled = markdown_disabled,
            color,
            utf8,
            "markdown renderer state for this stream"
        );
        Self {
            first_chunk: true,
            renderer: (!markdown_disabled)
                .then(|| crate::tui::markdown::MarkdownRenderer::new(color, utf8)),
        }
    }
}

pub(super) fn streaming_reply_needs_prefix(first_chunk: bool, stream_output: bool) -> bool {
    first_chunk && stream_output
}

pub(super) fn streaming_reply_needs_trailing_newline(
    first_chunk: bool,
    stream_output: bool,
) -> bool {
    stream_output && !first_chunk
}

pub(super) fn handle_streaming_assistant_chunk(
    render_state: &mut StreamingReplyRenderState,
    chunk: &str,
    stream_output: bool,
    stop_signal: Option<&SpinnerStopSignal>,
    interrupt_flag: &InterruptFlag,
) -> Result<(), String> {
    if interrupt_flag.is_set() {
        if let Some(sig) = stop_signal {
            sig.trigger();
        }
        return Err(USER_INTERRUPT_ERROR.to_string());
    }
    if render_state.first_chunk {
        if streaming_reply_needs_prefix(render_state.first_chunk, stream_output) {
            if let Some(sig) = stop_signal {
                sig.trigger();
            }
            write_stdout_rendered("assistant> ", false);
        }
        render_state.first_chunk = false;
    }
    if let Some(renderer) = render_state.renderer.as_mut() {
        let out = renderer.push_chunk(chunk);
        if !out.is_empty() && stream_output {
            write_stdout_rendered(&out, false);
        }
    } else if stream_output {
        write_stdout_rendered(chunk, false);
    }
    Ok(())
}

pub(super) fn finish_streaming_assistant_reply(
    render_state: &mut StreamingReplyRenderState,
    stream_output: bool,
) {
    if let Some(renderer) = render_state.renderer.as_mut() {
        let tail = renderer.flush();
        if !tail.is_empty() && stream_output {
            write_stdout_rendered(&tail, false);
        }
    }
    if streaming_reply_needs_trailing_newline(render_state.first_chunk, stream_output) {
        write_stdout_rendered("", true);
    }
}
