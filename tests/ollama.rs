mod support;

use anvil::models::ollama::OllamaClient;
use anvil::models::tool_calling::{NativeModelResponse, NativeToolSpec, ToolUseOptions};
use support::{json_response, ndjson_response, spawn_server};

#[tokio::test]
async fn ollama_health_hits_version_endpoint() {
    let server = spawn_server(vec![json_response(r#"{"version":"0.17.7"}"#)]);
    let client = OllamaClient::new(server.url()).unwrap();

    client.health().await.unwrap();

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/api/version");
}

#[tokio::test]
async fn ollama_list_models_returns_names() {
    let server = spawn_server(vec![json_response(
        r#"{"models":[{"name":"qwen3.5:35b"},{"name":"llama3.3"}]}"#,
    )]);
    let client = OllamaClient::new(server.url()).unwrap();

    let models = client.list_models().await.unwrap();

    assert_eq!(
        models,
        vec!["qwen3.5:35b".to_string(), "llama3.3".to_string()]
    );
}

#[tokio::test]
async fn ollama_chat_posts_non_streaming_payload() {
    let server = spawn_server(vec![json_response(
        r#"{"message":{"content":"hello from ollama"}}"#,
    )]);
    let client = OllamaClient::new(server.url()).unwrap();

    let text = client.chat("qwen3.5:35b", "Say hello").await.unwrap();

    assert_eq!(text, "hello from ollama");
    let requests = server.requests();
    assert_eq!(requests[0].path, "/api/chat");
    assert!(requests[0].body.contains(r#""stream":false"#));
    assert!(requests[0].body.contains(r#""model":"qwen3.5:35b""#));
}

#[tokio::test]
async fn ollama_chat_stream_concatenates_ndjson_chunks() {
    let server = spawn_server(vec![ndjson_response(&[
        r#"{"message":{"content":"hello "},"done":false}"#,
        r#"{"message":{"content":"world"},"done":false}"#,
        r#"{"done":true}"#,
    ])]);
    let client = OllamaClient::new(server.url()).unwrap();

    let text = client.chat_stream("qwen3.5:35b", "stream").await.unwrap();

    assert_eq!(text, "hello world");
    let requests = server.requests();
    assert!(requests[0].body.contains(r#""stream":true"#));
}

#[tokio::test]
async fn ollama_chat_with_tools_returns_structured_tool_calls() {
    let server = spawn_server(vec![json_response(
        r#"{"message":{"content":"","tool_calls":[{"function":{"name":"list_dir","arguments":{"path":"sandbox"}}}]}}"#,
    )]);
    let client = OllamaClient::new(server.url()).unwrap();
    let tools = vec![NativeToolSpec {
        name: "list_dir",
        description: "List entries in a directory",
        input_schema: serde_json::json!({"type":"object","properties":{"path":{"type":"string"}}}),
    }];

    let response = client
        .chat_with_tools(
            "qwen3.5:35b",
            "inspect sandbox",
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
            assert_eq!(calls[0].name, "list_dir");
        }
        other => panic!("expected tool calls, got {other:?}"),
    }
    let requests = server.requests();
    assert!(requests[0].body.contains(r#""tools":[{"type":"function""#));
    assert!(requests[0].body.contains(r#""temperature":0.2"#));
    assert!(requests[0].body.contains(r#""num_ctx":48000"#));
    assert!(requests[0].body.contains(r#""keep_alive":-1"#));
}
