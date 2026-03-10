mod support;

use anvil::models::lm_studio::LmStudioClient;
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
