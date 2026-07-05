use anvil::ollama::client::{
    OllamaClient, parse_chat_response, parse_generate_response, parse_tags_response,
    should_use_native_tool_calls,
};
use anvil::session::store::ConversationMessage;
use std::io::Read;
use std::net::TcpListener;
use std::time::{Duration, Instant};

#[test]
fn parses_tags_and_generate_payloads() {
    let tags = parse_tags_response(
        "{\"models\":[{\"name\":\"qwen3:8b\",\"details\":{}},{\"name\":\"gemma4:31b\",\"details\":{}}]}",
    )
    .unwrap();
    assert_eq!(tags, vec!["qwen3:8b", "gemma4:31b"]);

    let body = "{\"response\":\"Here you go.<anvil_tool_call>{\\\"name\\\":\\\"Read\\\",\\\"arguments\\\":{\\\"path\\\":\\\"README.md\\\"}}</anvil_tool_call>\"}";
    let reply = parse_generate_response(body, &["Read".to_string(), "Write".to_string()]).unwrap();
    assert_eq!(reply.tool_calls.len(), 1);
    assert_eq!(reply.tool_calls[0].name, "Read");
    assert_eq!(reply.tool_calls[0].arguments["path"], "README.md");
    assert!(reply.content.starts_with("Here you go."));
}

#[test]
fn native_tools_are_allowlisted_for_supported_models() {
    assert!(!should_use_native_tool_calls("qwen3.5:122b"));
    assert!(should_use_native_tool_calls("qwen3.6:27b-coding-nvfp4"));
    assert!(!should_use_native_tool_calls("qwen3:8b"));
    assert!(!should_use_native_tool_calls("gemma4:31b"));
    assert!(!should_use_native_tool_calls(""));
}

#[test]
fn parses_native_chat_tool_calls() {
    let body = r#"{
      "message": {
        "content": "",
        "tool_calls": [
          {
            "id": "call_write",
            "function": {
              "name": "Write",
              "arguments": {
                "path": "hello.txt",
                "content": "HELLO"
              }
            }
          }
        ]
      }
    }"#;
    let reply = parse_chat_response(body, &["Write".to_string()]).unwrap();
    assert_eq!(reply.tool_calls.len(), 1);
    assert_eq!(reply.tool_calls[0].name, "Write");
    assert_eq!(reply.tool_calls[0].arguments["content"], "HELLO");
}

fn stalled_provider_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let mut buf = [0_u8; 1024];
            let _ = stream.read(&mut buf);
            std::thread::sleep(Duration::from_secs(6));
        }
    });
    format!("http://{addr}")
}

#[test]
fn non_streaming_transport_timeout_is_provider_turn_timeout() {
    let client = OllamaClient::new_with_timeout_and_options(stalled_provider_url(), 1, 1024, 32)
        .expect("client");
    let messages = vec![ConversationMessage::user("hello".to_string())];

    let started = Instant::now();
    let err = client.chat_text("qwen-test", &messages).unwrap_err();

    assert!(err.contains("provider_turn_timeout"), "got: {err}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "transport timeout was not bounded: {:?}",
        started.elapsed()
    );
}

#[test]
fn streaming_transport_timeout_is_provider_turn_timeout() {
    let client = OllamaClient::new_with_timeout_and_options(stalled_provider_url(), 1, 1024, 32)
        .expect("client");
    let messages = vec![ConversationMessage::user("hello".to_string())];

    let started = Instant::now();
    let err = client
        .chat_streaming("qwen-test", &messages, &[], |_| Ok(()))
        .unwrap_err();

    assert!(err.contains("provider_turn_timeout"), "got: {err}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "streaming transport timeout was not bounded: {:?}",
        started.elapsed()
    );
}
