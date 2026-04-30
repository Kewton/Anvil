//! Issue #472 A/B eval harness — integration smoke suite.
//!
//! Tests the closure-DI env gate (`auto_test_disabled`) and the token-usage
//! fields added to `AssistantReply` via `parse_generate_response` /
//! `parse_chat_response`. No Ollama, no cargo, no network required.

use anvil::agent::loop_run::auto_test_disabled;
use anvil::ollama::client::{AssistantReply, parse_chat_response, parse_generate_response};

// ---------------------------------------------------------------------------
// auto_test_disabled gate tests
// ---------------------------------------------------------------------------

#[test]
fn auto_test_disabled_gate_set() {
    let disabled = auto_test_disabled(|k| {
        if k == "ANVIL_NO_AUTO_TEST" {
            Ok("1".to_string())
        } else {
            Err(std::env::VarError::NotPresent)
        }
    });
    assert!(
        disabled,
        "gate should be disabled when env var is non-empty"
    );
}

#[test]
fn auto_test_disabled_gate_unset() {
    let disabled = auto_test_disabled(|_k| Err(std::env::VarError::NotPresent));
    assert!(!disabled, "gate should be active when env var is absent");
}

#[test]
fn auto_test_disabled_gate_empty_value() {
    let disabled = auto_test_disabled(|k| {
        if k == "ANVIL_NO_AUTO_TEST" {
            Ok(String::new())
        } else {
            Err(std::env::VarError::NotPresent)
        }
    });
    assert!(
        !disabled,
        "gate should be active when env var is set to empty string"
    );
}

// ---------------------------------------------------------------------------
// Token usage in parse_generate_response
// ---------------------------------------------------------------------------

#[test]
fn parse_generate_response_emits_token_usage() {
    let body = r#"{
      "response": "hello",
      "done_reason": "stop",
      "prompt_eval_count": 42,
      "eval_count": 17
    }"#;
    let reply: AssistantReply = parse_generate_response(body, &[]).expect("parse should succeed");
    assert_eq!(reply.prompt_tokens, Some(42));
    assert_eq!(reply.completion_tokens, Some(17));
}

#[test]
fn parse_generate_response_token_usage_absent() {
    let body = r#"{
      "response": "hello",
      "done_reason": "stop"
    }"#;
    let reply: AssistantReply = parse_generate_response(body, &[]).expect("parse should succeed");
    assert_eq!(reply.prompt_tokens, None);
    assert_eq!(reply.completion_tokens, None);
}

// ---------------------------------------------------------------------------
// Token usage in parse_chat_response
// ---------------------------------------------------------------------------

#[test]
fn parse_chat_response_emits_token_usage() {
    let body = r#"{
      "message": { "content": "hi", "tool_calls": [] },
      "done_reason": "stop",
      "prompt_eval_count": 100,
      "eval_count": 50
    }"#;
    let reply: AssistantReply = parse_chat_response(body, &[]).expect("parse should succeed");
    assert_eq!(reply.prompt_tokens, Some(100));
    assert_eq!(reply.completion_tokens, Some(50));
}

#[test]
fn parse_chat_response_token_usage_absent() {
    let body = r#"{
      "message": { "content": "hi", "tool_calls": [] },
      "done_reason": "stop"
    }"#;
    let reply: AssistantReply = parse_chat_response(body, &[]).expect("parse should succeed");
    assert_eq!(reply.prompt_tokens, None);
    assert_eq!(reply.completion_tokens, None);
}
