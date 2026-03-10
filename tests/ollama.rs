mod support;

use anvil::models::ollama::OllamaClient;
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
