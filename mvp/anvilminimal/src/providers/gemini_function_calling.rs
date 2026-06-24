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
    if tools.is_empty() {
        json!({
            "model": normalize_interactions_model(model),
            "input": render_text_input(messages),
        })
    } else {
        json!({
            "model": normalize_interactions_model(model),
            "store": false,
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
            "tool" => input.push(json!({
                "type": "function_result",
                "name": message.name.clone().unwrap_or_else(|| "tool".to_string()),
                "call_id": message.tool_call_id.clone().unwrap_or_else(|| message.name.clone().unwrap_or_else(|| "tool".to_string())),
                "result": [{"type":"text","text":message.content}],
            })),
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

fn parse_step(step: &Value, content: &mut String, calls: &mut Vec<ToolCall>) -> anyhow::Result<()> {
    let kind = step.get("type").and_then(Value::as_str).unwrap_or_default();
    match kind {
        "text" | "output_text" | "thought" => {
            if let Some(text) = step.get("text").and_then(Value::as_str) {
                content.push_str(text);
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
            let arguments = step.get("arguments").cloned().unwrap_or(Value::Null);
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
    fn parses_function_call_round_trip() {
        let reply = parse_interactions_response(
            r#"{"output":[{"type":"function_call","name":"Read","call_id":"c1","arguments":{"path":"a"}}]}"#,
        )
        .unwrap();
        assert_eq!(reply.tool_calls[0].id, "c1");
    }

    #[test]
    fn handles_missing_thought() {
        let reply = parse_interactions_response(r#"{"output_text":"done"}"#).unwrap();
        assert_eq!(reply.content, "done");
    }

    #[test]
    fn provider_error_is_error() {
        assert!(parse_interactions_response(r#"{"error":{"message":"bad"}}"#).is_err());
    }
}
