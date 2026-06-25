use serde_json::{Value, json};

use crate::state::{ConversationMessage, ToolCall};
use crate::tools::registry::ToolSpec;

use super::AssistantReply;
use super::parsing::sanitize_schema;

pub fn build_interactions_request(
    model: &str,
    messages: &[ConversationMessage],
    tools: &[ToolSpec],
    _max_predict: usize,
) -> Value {
    build_interactions_request_with_previous(model, messages, tools, _max_predict, None)
}

pub fn build_interactions_request_with_previous(
    model: &str,
    messages: &[ConversationMessage],
    tools: &[ToolSpec],
    _max_predict: usize,
    previous_interaction_id: Option<&str>,
) -> Value {
    if tools.is_empty() {
        json!({
            "model": normalize_interactions_model(model),
            "input": render_text_input(messages),
        })
    } else if let Some(previous_interaction_id) = previous_interaction_id {
        json!({
            "model": normalize_interactions_model(model),
            "previous_interaction_id": previous_interaction_id,
            "input": previous_interaction_input(messages),
            "tools": tools.iter().map(gemini_tool_declaration).collect::<Vec<_>>(),
        })
    } else {
        json!({
            "model": normalize_interactions_model(model),
            "input": gemini_input(messages),
            "tools": tools.iter().map(gemini_tool_declaration).collect::<Vec<_>>(),
        })
    }
}

fn normalize_interactions_model(model: &str) -> String {
    model.strip_prefix("models/").unwrap_or(model).to_string()
}

fn gemini_tool_declaration(tool: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "name": tool.function.name,
        "description": tool.function.description,
        "parameters": sanitize_schema(&tool.function.parameters),
    })
}

fn render_text_input(messages: &[ConversationMessage]) -> String {
    messages
        .iter()
        .filter(|message| message.role != "tool")
        .map(|message| {
            if message.role == "user" {
                message.content.clone()
            } else {
                format!("{}:\n{}", message.role, message.content)
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn gemini_input(messages: &[ConversationMessage]) -> Vec<Value> {
    let mut input = Vec::new();
    for message in messages {
        match message.role.as_str() {
            "assistant" => {
                if !message.content.trim().is_empty() {
                    input.push(json!({
                        "type":"thought",
                        "content":[{"type":"text","text":message.content}]
                    }));
                }
                for call in &message.tool_calls {
                    input.push(json!({
                        "type": "function_call",
                        "name": call.name,
                        "id": call.id,
                        "arguments": call.arguments,
                    }));
                }
            }
            "tool" => input.push(gemini_tool_result(message)),
            "user" => input.push(json!({"type":"user_input","content":message.content})),
            "system" | "developer" => input.push(json!({"type":"user_input","content":format!("{}\n{}", message.role, message.content)})),
            _ => input.push(json!({"type":"user_input","content":message.content})),
        }
    }
    if input.is_empty() {
        input.push(json!({"type":"user_input","content":""}));
    }
    input
}

fn trailing_tool_results(messages: &[ConversationMessage]) -> Vec<Value> {
    let mut tool_messages = messages
        .iter()
        .rev()
        .take_while(|message| message.role == "tool")
        .collect::<Vec<_>>();
    tool_messages.reverse();
    tool_messages
        .into_iter()
        .map(gemini_tool_result)
        .collect::<Vec<_>>()
}

fn previous_interaction_input(messages: &[ConversationMessage]) -> Vec<Value> {
    let tool_results = trailing_tool_results(messages);
    if !tool_results.is_empty() {
        return tool_results;
    }
    if let Some(message) = messages.iter().rev().find(|message| {
        matches!(
            message.role.as_str(),
            "user" | "system" | "developer" | "tool"
        )
    }) {
        return match message.role.as_str() {
            "tool" => vec![gemini_tool_result(message)],
            "system" | "developer" => {
                vec![
                    json!({"type":"user_input","content":format!("{}\n{}", message.role, message.content)}),
                ]
            }
            _ => vec![json!({"type":"user_input","content":message.content})],
        };
    }
    vec![json!({"type":"user_input","content":""})]
}

fn gemini_tool_result(message: &ConversationMessage) -> Value {
    json!({
        "type": "function_result",
        "name": message.name.clone().unwrap_or_else(|| "tool".to_string()),
        "call_id": message.tool_call_id.clone().unwrap_or_else(|| message.name.clone().unwrap_or_else(|| "tool".to_string())),
        "result": [{"type":"text","text":message.content}],
    })
}

pub fn parse_interactions_response(body: &str) -> anyhow::Result<AssistantReply> {
    let value: Value = serde_json::from_str(body)?;
    if let Some(error) = value.get("error") {
        anyhow::bail!("Gemini provider error: {error}");
    }
    let mut content = String::new();
    let mut calls = Vec::new();
    if let Some(text) = value.get("output_text").and_then(Value::as_str) {
        content.push_str(text);
    }
    if let Some(output) = value.get("output").and_then(Value::as_array) {
        for step in output {
            parse_step(step, &mut content, &mut calls)?;
        }
    }
    if let Some(steps) = value.get("steps").and_then(Value::as_array) {
        for step in steps {
            parse_step(step, &mut content, &mut calls)?;
        }
    }
    Ok(AssistantReply {
        content,
        tool_calls: calls,
        prompt_tokens: None,
        completion_tokens: None,
    })
}

pub fn interaction_id(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    value.get("id").and_then(Value::as_str).map(str::to_string)
}

fn parse_step(step: &Value, content: &mut String, calls: &mut Vec<ToolCall>) -> anyhow::Result<()> {
    let kind = step.get("type").and_then(Value::as_str).unwrap_or_default();
    match kind {
        "text" | "output_text" | "thought" => {
            if let Some(text) = step.get("text").and_then(Value::as_str) {
                content.push_str(text);
            }
        }
        "model_output" => {
            if let Some(items) = step.get("content").and_then(Value::as_array) {
                for item in items {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        content.push_str(text);
                    }
                }
            }
        }
        "function_call" => {
            let name = step
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("Gemini function_call missing name"))?;
            let id = step
                .get("id")
                .or_else(|| step.get("call_id"))
                .and_then(Value::as_str)
                .unwrap_or(name)
                .to_string();
            let arguments = normalize_function_arguments(step.get("arguments").cloned())?;
            calls.push(ToolCall {
                id,
                name: name.to_string(),
                arguments,
            });
        }
        _ => {}
    }
    Ok(())
}

fn normalize_function_arguments(value: Option<Value>) -> anyhow::Result<Value> {
    match value {
        Some(object @ Value::Object(_)) => Ok(object),
        Some(Value::String(raw)) => {
            let decoded: Value = serde_json::from_str(&raw).map_err(|err| {
                anyhow::anyhow!("Gemini function_call arguments are not valid JSON: {err}")
            })?;
            match decoded {
                Value::Object(_) => Ok(decoded),
                other => Err(anyhow::anyhow!(
                    "Gemini function_call arguments must decode to object, got {}",
                    json_type(&other)
                )),
            }
        }
        Some(Value::Null) | None => Err(anyhow::anyhow!("Gemini function_call missing arguments")),
        Some(other) => Err(anyhow::anyhow!(
            "Gemini function_call arguments must be object or JSON string, got {}",
            json_type(&other)
        )),
    }
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::registry::ToolRegistry;

    #[test]
    fn request_keeps_gemini_model_strings() {
        let body = build_interactions_request(
            "gemini-3.5-flash",
            &[],
            ToolRegistry::default().specs(),
            100,
        );
        assert_eq!(body["model"], "gemini-3.5-flash");
        let body = build_interactions_request(
            "gemini-3.1-flash-lite",
            &[],
            ToolRegistry::default().specs(),
            100,
        );
        assert_eq!(body["model"], "gemini-3.1-flash-lite");
    }

    #[test]
    fn no_tool_planner_request_uses_text_input() {
        let body = build_interactions_request(
            "models/gemini-3.5-flash",
            &[ConversationMessage::user("make a plan")],
            &[],
            100,
        );
        assert_eq!(body["model"], "gemini-3.5-flash");
        assert_eq!(body["input"], "make a plan");
        assert!(body.get("tools").is_none());
        assert!(body.get("store").is_none());
        assert!(body.get("generation_config").is_none());
    }

    #[test]
    fn stateful_tool_result_request_uses_previous_interaction_id() {
        let body = build_interactions_request_with_previous(
            "gemini-3.1-flash-lite",
            &[ConversationMessage::tool_result(
                "Write".to_string(),
                Some("call-1".to_string()),
                "ok".to_string(),
            )],
            ToolRegistry::default().specs(),
            100,
            Some("interaction-1"),
        );
        assert_eq!(body["previous_interaction_id"], "interaction-1");
        assert_eq!(body["input"][0]["type"], "function_result");
        assert_eq!(body["input"][0]["call_id"], "call-1");
        assert!(body.get("store").is_none());
    }

    #[test]
    fn stateful_next_user_request_sends_latest_user_input_only() {
        let body = build_interactions_request_with_previous(
            "gemini-3.1-flash-lite",
            &[
                ConversationMessage::user("old"),
                ConversationMessage::assistant("done", Vec::new()),
                ConversationMessage::user("new task"),
            ],
            ToolRegistry::default().specs(),
            100,
            Some("interaction-1"),
        );
        assert_eq!(body["previous_interaction_id"], "interaction-1");
        assert_eq!(body["input"][0]["type"], "user_input");
        assert_eq!(body["input"][0]["content"], "new task");
        assert_eq!(body["input"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn parses_function_call_round_trip() {
        let reply = parse_interactions_response(
            r#"{"output":[{"type":"function_call","name":"Read","call_id":"c1","arguments":{"path":"a"}}]}"#,
        )
        .unwrap();
        assert_eq!(reply.tool_calls[0].id, "c1");
    }

    #[test]
    fn parses_function_call_arguments_json_string() {
        let reply = parse_interactions_response(
            r#"{"output":[{"type":"function_call","name":"Grep","call_id":"c1","arguments":"{\"pattern\":\"TODO\"}"}]}"#,
        )
        .unwrap();
        assert_eq!(reply.tool_calls[0].arguments["pattern"], "TODO");
    }

    #[test]
    fn rejects_missing_function_call_arguments() {
        let err = parse_interactions_response(
            r#"{"output":[{"type":"function_call","name":"Grep","call_id":"c1"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("missing arguments"));
    }

    #[test]
    fn handles_missing_thought() {
        let reply = parse_interactions_response(r#"{"output_text":"done"}"#).unwrap();
        assert_eq!(reply.content, "done");
    }

    #[test]
    fn parses_model_output_content_text() {
        let reply = parse_interactions_response(
            r#"{"steps":[{"type":"thought","signature":"sig"},{"type":"model_output","content":[{"type":"text","text":"{\"goal\":\"g\",\"steps\":[]}"}]}]}"#,
        )
        .unwrap();
        assert_eq!(reply.content, r#"{"goal":"g","steps":[]}"#);
    }

    #[test]
    fn extracts_interaction_id() {
        assert_eq!(
            interaction_id(r#"{"id":"interaction-1","output_text":"done"}"#).as_deref(),
            Some("interaction-1")
        );
    }

    #[test]
    fn provider_error_is_error() {
        assert!(parse_interactions_response(r#"{"error":{"message":"bad"}}"#).is_err());
    }
}
