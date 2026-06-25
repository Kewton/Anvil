use std::path::{Path, PathBuf};

use anvilminimal::config::{Action, Config, Provider, load_api_key};
use anvilminimal::providers::ChatClient;
use anvilminimal::providers::gemini::GeminiClient;
use anvilminimal::providers::gemini_function_calling::build_interactions_request;
use anvilminimal::providers::ollama::OllamaClient;
use anvilminimal::providers::openai::{OpenAiClient, build_response_request};
use anvilminimal::state::ConversationMessage;
use anvilminimal::tools::registry::ToolRegistry;

#[test]
#[ignore]
fn live_openai_request_shape_uses_smoke_model() {
    if std::env::var("ANVIL_LIVE_PROVIDER_TESTS").ok().as_deref() != Some("1") {
        return;
    }
    if find_workspace_with_key("OPENAI_API_KEY").is_none() {
        return;
    }
    let model =
        std::env::var("ANVIL_OPENAI_SMOKE_MODEL").unwrap_or_else(|_| "gpt-5.4-mini".to_string());
    let body = build_response_request(&model, &[], ToolRegistry::default().specs(), true, 64);
    assert_eq!(body["model"], model);
}

#[test]
#[ignore]
fn live_openai_responses_no_tool_http_smoke() {
    if std::env::var("ANVIL_LIVE_PROVIDER_TESTS").ok().as_deref() != Some("1") {
        return;
    }
    let Some(workspace_root) = find_workspace_with_key("OPENAI_API_KEY") else {
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = Config {
        workspace_root,
        state_dir: tmp.path().join("state"),
        eval_events_path: None,
        yes: true,
        offline: false,
        context_budget: 4096,
        model: std::env::var("ANVIL_OPENAI_SMOKE_MODEL")
            .unwrap_or_else(|_| "gpt-5.4-mini".to_string()),
        provider: Provider::Openai,
        planner_model: "unused".to_string(),
        planner_provider: Provider::Openai,
        ollama_host: "http://127.0.0.1:11434".to_string(),
        num_predict: 64,
        max_iterations: 1,
        chat_timeout_secs: 30,
        chat_retries: 0,
        resume: None,
        fresh_session: true,
        no_footer: false,
        profile: "default".to_string(),
        style: "balanced".to_string(),
        action: Action::Prompt(String::new()),
    };
    let mut client = OpenAiClient::from_env(&config).expect("openai client");
    client
        .chat(
            &config.model,
            &[ConversationMessage::user("Reply with exactly OK.")],
            &[],
            false,
        )
        .expect("OpenAI no-tool Responses API smoke");
}

#[test]
#[ignore]
fn live_gemini_request_shape_uses_smoke_model() {
    if std::env::var("ANVIL_LIVE_PROVIDER_TESTS").ok().as_deref() != Some("1") {
        return;
    }
    if find_workspace_with_key("GEMINI_API_KEY").is_none() {
        return;
    }
    let model = std::env::var("ANVIL_GEMINI_SMOKE_MODEL")
        .unwrap_or_else(|_| "gemini-3.1-flash-lite".to_string());
    let body = build_interactions_request(&model, &[], ToolRegistry::default().specs(), 64);
    assert_eq!(body["model"], model);
}

#[test]
#[ignore]
fn live_gemini_interactions_no_tool_http_smoke() {
    if std::env::var("ANVIL_LIVE_PROVIDER_TESTS").ok().as_deref() != Some("1") {
        return;
    }
    let Some(workspace_root) = find_workspace_with_key("GEMINI_API_KEY") else {
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let config = Config {
        workspace_root,
        state_dir: tmp.path().join("state"),
        eval_events_path: None,
        yes: true,
        offline: false,
        context_budget: 4096,
        model: "unused".to_string(),
        provider: Provider::Ollama,
        planner_model: std::env::var("ANVIL_GEMINI_SMOKE_MODEL")
            .unwrap_or_else(|_| "gemini-3.5-flash".to_string()),
        planner_provider: Provider::Gemini,
        ollama_host: "http://127.0.0.1:11434".to_string(),
        num_predict: 64,
        max_iterations: 1,
        chat_timeout_secs: 30,
        chat_retries: 0,
        resume: None,
        fresh_session: true,
        no_footer: false,
        profile: "default".to_string(),
        style: "balanced".to_string(),
        action: Action::Prompt(String::new()),
    };
    let mut client = GeminiClient::from_env(&config).expect("gemini client");
    client
        .chat(
            &config.planner_model,
            &[ConversationMessage::user("Reply with exactly OK.")],
            &[],
            false,
        )
        .expect("Gemini no-tool Interactions API smoke");
}

#[test]
#[ignore]
fn live_ollama_tags_http_smoke() {
    if std::env::var("ANVIL_LIVE_PROVIDER_TESTS").ok().as_deref() != Some("1") {
        return;
    }
    let host =
        std::env::var("ANVIL_OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
    let client = OllamaClient::new(host, 10, 64, 0).expect("ollama client");
    let models = client.list_models().expect("Ollama /api/tags smoke");
    assert!(!models.is_empty());
}

#[test]
#[ignore]
fn live_ollama_chat_no_tool_http_smoke() {
    if std::env::var("ANVIL_LIVE_PROVIDER_TESTS").ok().as_deref() != Some("1") {
        return;
    }
    let host =
        std::env::var("ANVIL_OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
    let model = std::env::var("ANVIL_OLLAMA_SMOKE_MODEL")
        .unwrap_or_else(|_| "qwen3.6:27b-coding-nvfp4".to_string());
    let mut client = OllamaClient::new(host, 30, 64, 0).expect("ollama client");
    client
        .chat(
            &model,
            &[ConversationMessage::user("Reply with exactly OK.")],
            &[],
            false,
        )
        .expect("Ollama /api/chat no-tool smoke");
}

fn find_workspace_with_key(name: &str) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    for path in cwd.ancestors() {
        if load_api_key(path, name).is_ok() {
            return Some(path.to_path_buf());
        }
    }
    if load_api_key(Path::new("."), name).is_ok() {
        Some(PathBuf::from("."))
    } else {
        None
    }
}
