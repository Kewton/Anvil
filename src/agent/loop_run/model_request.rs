use std::path::Path;
use std::time::Duration;

use crate::model_capabilities::model_capabilities;
use crate::ollama::client::{AssistantReply, OllamaClient};
use crate::session::store::ConversationMessage;
use crate::tools::registry::ToolSpec;

use super::tool_history::focused_edit_target_already_read;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AssistantRequestPlan {
    pub(super) focused_edit_timeout_override: Option<u64>,
    pub(super) focused_edit_max_predict_override: Option<usize>,
    pub(super) use_streaming_transport: bool,
}

pub(super) fn should_use_streaming_transport(
    model: &str,
    _native_tools_enabled: bool,
    stream_output: bool,
    stdin_is_terminal: bool,
) -> bool {
    let wants_streaming = stream_output || stdin_is_terminal;
    if !wants_streaming {
        return false;
    }

    if !model_capabilities(model).streaming_tool_calls {
        return false;
    }

    true
}

pub(super) fn non_streaming_assistant_reply_timeout_secs(
    model: &str,
    _native_tools_enabled: bool,
    default_timeout_secs: u64,
) -> u64 {
    model_capabilities(model)
        .non_streaming_hard_timeout_secs
        .unwrap_or(default_timeout_secs)
}

pub(super) fn effective_non_streaming_timeout_secs(
    model: &str,
    native_tools_enabled: bool,
    default_timeout_secs: u64,
    timeout_override_secs: Option<u64>,
) -> u64 {
    let model_timeout = non_streaming_assistant_reply_timeout_secs(
        model,
        native_tools_enabled,
        default_timeout_secs,
    );
    match timeout_override_secs {
        Some(override_secs) => override_secs,
        None => model_timeout,
    }
}

pub(super) fn focused_edit_timeout_override_secs(
    model: &str,
    messages: &[ConversationMessage],
    target: Option<&Path>,
    work_root: &Path,
) -> Option<u64> {
    let target = target?;
    let focused_edit = model_capabilities(model).focused_edit?;
    Some(
        if focused_edit_target_already_read(messages, target, work_root) {
            focused_edit.post_read_timeout_secs
        } else {
            focused_edit.pre_read_timeout_secs
        },
    )
}

pub(super) fn focused_edit_max_predict_override(
    model: &str,
    messages: &[ConversationMessage],
    target: Option<&Path>,
    work_root: &Path,
) -> Option<usize> {
    let target = target?;
    let focused_edit = model_capabilities(model).focused_edit?;
    Some(
        if focused_edit_target_already_read(messages, target, work_root) {
            focused_edit.post_read_max_predict
        } else {
            focused_edit.pre_read_max_predict
        },
    )
}

pub(super) fn build_assistant_request_plan(
    model: &str,
    native_tools_enabled: bool,
    stream_output: bool,
    stdin_is_terminal: bool,
    messages: &[ConversationMessage],
    target: Option<&Path>,
    work_root: &Path,
) -> AssistantRequestPlan {
    let focused_edit_timeout_override =
        focused_edit_timeout_override_secs(model, messages, target, work_root);
    let focused_edit_max_predict_override =
        focused_edit_max_predict_override(model, messages, target, work_root);
    let force_non_streaming_for_focused_edit =
        focused_edit_timeout_override.is_some() || focused_edit_max_predict_override.is_some();
    let use_streaming_transport = !force_non_streaming_for_focused_edit
        && should_use_streaming_transport(
            model,
            native_tools_enabled,
            stream_output,
            stdin_is_terminal,
        );
    AssistantRequestPlan {
        focused_edit_timeout_override,
        focused_edit_max_predict_override,
        use_streaming_transport,
    }
}

pub(super) fn request_non_streaming_assistant_reply(
    client: &OllamaClient,
    model: &str,
    messages: &[ConversationMessage],
    tool_specs: &[ToolSpec],
    native_tools_enabled: bool,
    timeout_override_secs: Option<u64>,
    max_predict_override: Option<usize>,
) -> Result<AssistantReply, String> {
    let client = if let Some(max_predict) = max_predict_override {
        client.clone_with_overrides(client.timeout_secs(), max_predict)?
    } else {
        client.clone()
    };
    let model = model.to_string();
    let tool_specs = tool_specs.to_vec();
    let owned_messages = messages.to_vec();
    let timeout = Duration::from_secs(effective_non_streaming_timeout_secs(
        &model,
        native_tools_enabled,
        client.timeout_secs(),
        timeout_override_secs,
    ));
    let (tx, rx) = std::sync::mpsc::sync_channel(1);

    std::thread::spawn(move || {
        let result =
            client.chat_with_mode(&model, &owned_messages, &tool_specs, native_tools_enabled);
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "assistant reply timed out after {}s",
            timeout.as_secs()
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err("assistant reply worker disconnected".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::store::ConversationMessage;
    use std::path::Path;

    #[test]
    fn streaming_transport_requires_streaming_capable_model_and_demand() {
        assert!(!should_use_streaming_transport(
            "qwen3.5:9b",
            true,
            false,
            false
        ));
        assert!(!should_use_streaming_transport(
            "qwen3.5:122b",
            true,
            true,
            true
        ));
        assert!(should_use_streaming_transport(
            "qwen3.6:27b-coding-nvfp4",
            true,
            true,
            false
        ));
    }

    #[test]
    fn timeout_override_wins_over_model_default() {
        assert_eq!(
            effective_non_streaming_timeout_secs("qwen3.5:122b", true, 120, Some(45)),
            45
        );
    }

    #[test]
    fn focused_edit_overrides_require_target() {
        let messages: Vec<ConversationMessage> = Vec::new();
        assert_eq!(
            focused_edit_timeout_override_secs("qwen3.5:122b", &messages, None, Path::new(".")),
            None
        );
        assert_eq!(
            focused_edit_max_predict_override("qwen3.5:122b", &messages, None, Path::new(".")),
            None
        );
    }

    #[test]
    fn request_plan_forces_non_streaming_for_focused_edit() {
        let messages: Vec<ConversationMessage> = Vec::new();
        let plan = build_assistant_request_plan(
            "qwen3.5:122b",
            true,
            true,
            true,
            &messages,
            Some(Path::new("src/main.rs")),
            Path::new("."),
        );

        assert!(plan.focused_edit_timeout_override.is_some());
        assert!(plan.focused_edit_max_predict_override.is_some());
        assert!(!plan.use_streaming_transport);
    }

    #[test]
    fn request_plan_uses_streaming_when_no_focused_edit_override_exists() {
        let messages: Vec<ConversationMessage> = Vec::new();
        let plan = build_assistant_request_plan(
            "qwen3.6:27b-coding-nvfp4",
            true,
            true,
            true,
            &messages,
            None,
            Path::new("."),
        );

        assert!(plan.use_streaming_transport);
    }
}
