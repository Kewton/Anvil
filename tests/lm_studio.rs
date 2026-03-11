mod support;

use anvil::models::lm_studio::LmStudioClient;
use anvil::models::tool_calling::{NativeModelResponse, NativeToolSpec, ToolUseOptions};
use support::{json_response, spawn_server, sse_response};

#[tokio::test]
async fn lm_studio_lists_models() {
    let server = spawn_server(vec![json_response(
        r#"{"data":[{"id":"qwen3.5:35b"},{"id":"mistral-small"}]}"#,
    )]);
    let client = LmStudioClient::new(server.url()).unwrap();

    let models = client.list_models().await.unwrap();

    assert_eq!(
        models,
        vec!["qwen3.5:35b".to_string(), "mistral-small".to_string()]
    );
}

#[tokio::test]
async fn lm_studio_chat_and_stream_work() {
    let server = spawn_server(vec![
        json_response(r#"{"choices":[{"message":{"content":"hello from lm studio"}}]}"#),
        sse_response(&[
            r#"{"choices":[{"delta":{"content":"hello "}}]}"#,
            r#"{"choices":[{"delta":{"content":"stream"}}]}"#,
        ]),
    ]);
    let client = LmStudioClient::new(server.url()).unwrap();

    let chat = client.chat("qwen3.5:35b", "Say hi").await.unwrap();
    let stream = client.chat_stream("qwen3.5:35b", "Stream").await.unwrap();

    assert_eq!(chat, "hello from lm studio");
    assert_eq!(stream, "hello stream");
}

#[tokio::test]
async fn lm_studio_chat_with_tools_returns_structured_tool_calls() {
    let server = spawn_server(vec![json_response(
        r#"{"choices":[{"message":{"content":null,"tool_calls":[{"id":"call_1","function":{"name":"mkdir","arguments":{"path":"sandbox/test"}}}]}}]}"#,
    )]);
    let client = LmStudioClient::new(server.url()).unwrap();
    let tools = vec![NativeToolSpec {
        name: "mkdir",
        description: "Create a directory recursively",
        input_schema: serde_json::json!({"type":"object","properties":{"path":{"type":"string"}}}),
    }];

    let response = client
        .chat_with_tools(
            "qwen3.5:35b",
            "make sandbox",
            &tools,
            ToolUseOptions {
                temperature: 0.2,
                max_context_tokens: 48_000,
                keep_alive: true,
            },
        )
        .await
        .unwrap();

    match response {
        NativeModelResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].name, "mkdir");
            assert_eq!(calls[0].id.as_deref(), Some("call_1"));
        }
        other => panic!("expected tool calls, got {other:?}"),
    }
    let requests = server.requests();
    assert!(requests[0].body.contains(r#""tools":[{"type":"function""#));
    assert!(requests[0].body.contains(r#""temperature":0.2"#));
}
