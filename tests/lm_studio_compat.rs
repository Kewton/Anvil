mod support;

use anvil::models::lm_studio::LmStudioClient;
use support::{json_response, spawn_server, sse_response};

#[tokio::test]
async fn lm_studio_handles_openai_compatible_payloads() {
    let server = spawn_server(vec![
        json_response(r#"{"choices":[{"message":{"content":"plain completion"}}]}"#),
        sse_response(&[
            r#"{"choices":[{"delta":{"content":"chunk-1"}}]}"#,
            r#"{"choices":[{"delta":{"content":"-chunk-2"}}]}"#,
            "[DONE]",
        ]),
    ]);
    let client = LmStudioClient::new(server.url()).unwrap();

    let non_stream = client.chat("qwen3.5:35b", "plain").await.unwrap();
    let stream = client.chat_stream("qwen3.5:35b", "stream").await.unwrap();

    assert_eq!(non_stream, "plain completion");
    assert_eq!(stream, "chunk-1-chunk-2");
}
