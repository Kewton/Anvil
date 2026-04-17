use serde_json::json;

use crate::logging;
use crate::ollama::parsing::{AssistantReply, parse_chat_response, tool_names, truncate_for_log};
use crate::ollama::transport::ChatTransport;
use crate::session::store::ConversationMessage;
use crate::tools::registry::ToolSpec;

pub(crate) fn salvage_malformed_tool_call(
    transport: &ChatTransport<'_>,
    model: &str,
    messages: &[ConversationMessage],
    tools: &[ToolSpec],
) -> Result<AssistantReply, String> {
    let mut salvage_messages = messages.to_vec();
    salvage_messages.push(ConversationMessage::system(salvage_prompt().to_string()));

    logging::log_llm_event(
        "ollama.chat.salvage_request",
        json!({
            "model": model,
            "tools": tools.iter().map(|tool| tool.function.name.clone()).collect::<Vec<_>>(),
        }),
    );

    let response = transport
        .send_chat_request(model, &salvage_messages, None, false, 0.2)
        .map_err(|err| format!("failed salvage chat retry: {err}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "salvage Ollama /api/chat failed: {}",
            response.status()
        ));
    }

    let body = response
        .text()
        .map_err(|err| format!("failed to decode salvage chat response: {err}"))?;
    logging::log_llm_event(
        "ollama.chat.salvage_response_raw",
        json!({
            "model": model,
            "body": truncate_for_log(&body, 200_000),
        }),
    );
    parse_chat_response(&body, &tool_names(tools))
}

fn salvage_prompt() -> &'static str {
    "The previous native tool call response was malformed and rejected by the runtime parser. Retry immediately. Return exactly one valid next action. Do not emit native tool_calls. If a tool is needed, emit a single <anvil_tool_call>{\"name\":\"Tool\",\"arguments\":{\"key\":\"value\"}}</anvil_tool_call> block with valid JSON and no extra prose."
}
