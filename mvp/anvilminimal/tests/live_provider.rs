use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anvilminimal::config::{Action, Config, Provider, load_api_key};
use anvilminimal::providers::ChatClient;
use anvilminimal::providers::gemini::GeminiClient;
use anvilminimal::providers::gemini_function_calling::{
    build_interactions_request, parse_interactions_response,
};
use anvilminimal::providers::ollama::OllamaClient;
use anvilminimal::providers::ollama::parse_chat_response;
use anvilminimal::providers::openai::{
    OpenAiClient, build_response_request, parse_openai_response,
};
use anvilminimal::state::ConversationMessage;
use anvilminimal::tools::registry::ToolRegistry;
use serde_json::{Value, json};

static PROVIDER_PROBE_EVENT_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn planner_live_provider_smoke_skips_without_keys() {
    if std::env::var("ANVIL_LIVE_PROVIDER_TESTS").ok().as_deref() == Some("1") {
        let _ = find_workspace_with_key("OPENAI_API_KEY");
        let _ = find_workspace_with_key("GEMINI_API_KEY");
    }
}

#[test]
fn planner_live_openai_gemini_json_contract() {
    if std::env::var("ANVIL_LIVE_PROVIDER_TESTS").ok().as_deref() != Some("1") {
        return;
    }
    let Some(openai_root) = find_workspace_with_key("OPENAI_API_KEY") else {
        return;
    };
    let Some(gemini_root) = find_workspace_with_key("GEMINI_API_KEY") else {
        return;
    };
    let goal = "Build a Python markdown heading linter with unit tests.";
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut openai_config = smoke_config(tmp.path(), openai_root, Provider::Openai);
    openai_config.planner_model =
        std::env::var("ANVIL_OPENAI_SMOKE_MODEL").unwrap_or_else(|_| "gpt-5.4-mini".to_string());
    openai_config.planner_provider = Provider::Openai;
    let mut openai = OpenAiClient::from_env(&openai_config).expect("openai client");
    let openai_plan = anvilminimal::planner::generate_step_plan(&mut openai, goal, &openai_config)
        .expect("OpenAI planner JSON contract");
    assert!(!openai_plan.steps.is_empty());

    let tmp = tempfile::tempdir().expect("tempdir");
    let mut gemini_config = smoke_config(tmp.path(), gemini_root, Provider::Gemini);
    gemini_config.planner_model = std::env::var("ANVIL_GEMINI_SMOKE_MODEL")
        .unwrap_or_else(|_| "gemini-3.5-flash".to_string());
    gemini_config.planner_provider = Provider::Gemini;
    let mut gemini = GeminiClient::from_env(&gemini_config).expect("gemini client");
    let gemini_plan = anvilminimal::planner::generate_step_plan(&mut gemini, goal, &gemini_config)
        .expect("Gemini planner JSON contract");
    assert!(!gemini_plan.steps.is_empty());
}

#[test]
fn provider_probe_openai_tool_args_shape_skips_without_key() {
    if !provider_probe_enabled() {
        return;
    }
    let Some(openai_root) = find_workspace_with_key("OPENAI_API_KEY") else {
        record_provider_probe(json!({
            "provider": "openai",
            "probe": "tool_args_shape",
            "status": "skipped",
            "reason": "missing_openai_api_key",
        }));
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut config = smoke_config(tmp.path(), openai_root, Provider::Openai);
    config.model =
        std::env::var("ANVIL_OPENAI_SMOKE_MODEL").unwrap_or_else(|_| "gpt-5.4-mini".to_string());
    let mut client = OpenAiClient::from_env(&config).expect("openai client");
    let result = client.chat(
        &config.model,
        &[ConversationMessage::user(
            "Use the Write tool exactly once with path provider-probe.txt and content provider probe ok. Do not answer in plain text."
                .to_string(),
        )],
        ToolRegistry::default().specs(),
        true,
    );
    match result {
        Ok(reply) => {
            let tool_names = reply
                .tool_calls
                .iter()
                .map(|call| call.name.clone())
                .collect::<Vec<_>>();
            let args_shape_ok = reply
                .tool_calls
                .iter()
                .all(|call| call.arguments.as_object().is_some());
            record_provider_probe(json!({
                "provider": "openai",
                "model": config.model,
                "probe": "tool_args_shape",
                "status": if args_shape_ok && !reply.tool_calls.is_empty() { "passed" } else { "failed" },
                "tool_calls": reply.tool_calls.len(),
                "tool_names": tool_names,
                "arguments_shape": if args_shape_ok { "object" } else { "non_object_or_missing" },
            }));
            assert!(
                !reply.tool_calls.is_empty(),
                "OpenAI probe returned no tool calls"
            );
            assert!(args_shape_ok, "OpenAI probe returned non-object arguments");
        }
        Err(err) => {
            record_provider_probe(json!({
                "provider": "openai",
                "model": config.model,
                "probe": "tool_args_shape",
                "status": "failed",
                "error_kind": "provider_error",
                "message": err.to_string(),
            }));
            panic!("OpenAI provider probe failed: {err}");
        }
    }
}

#[test]
fn provider_probe_gemini_function_calling_schema_skips_without_key() {
    if !provider_probe_enabled() {
        return;
    }
    let Some(gemini_root) = find_workspace_with_key("GEMINI_API_KEY") else {
        record_provider_probe(json!({
            "provider": "gemini",
            "probe": "function_calling_schema",
            "status": "skipped",
            "reason": "missing_gemini_api_key",
        }));
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut config = smoke_config(tmp.path(), gemini_root, Provider::Gemini);
    config.planner_model = std::env::var("ANVIL_GEMINI_SMOKE_MODEL")
        .unwrap_or_else(|_| "gemini-3.5-flash".to_string());
    let mut client = GeminiClient::from_env(&config).expect("gemini client");
    let result = client.chat(
        &config.planner_model,
        &[ConversationMessage::user(
            "Use the Write function exactly once with path provider-probe.txt and content provider probe ok. Do not answer in plain text."
                .to_string(),
        )],
        ToolRegistry::default().specs(),
        true,
    );
    match result {
        Ok(reply) => {
            let tool_names = reply
                .tool_calls
                .iter()
                .map(|call| call.name.clone())
                .collect::<Vec<_>>();
            let args_shape_ok = reply
                .tool_calls
                .iter()
                .all(|call| call.arguments.as_object().is_some());
            record_provider_probe(json!({
                "provider": "gemini",
                "model": config.planner_model,
                "probe": "function_calling_schema",
                "status": if args_shape_ok && !reply.tool_calls.is_empty() { "passed" } else { "failed" },
                "tool_calls": reply.tool_calls.len(),
                "tool_names": tool_names,
                "arguments_shape": if args_shape_ok { "object" } else { "non_object_or_missing" },
            }));
            assert!(
                !reply.tool_calls.is_empty(),
                "Gemini probe returned no tool calls"
            );
            assert!(args_shape_ok, "Gemini probe returned non-object arguments");
        }
        Err(err) => {
            record_provider_probe(json!({
                "provider": "gemini",
                "model": config.planner_model,
                "probe": "function_calling_schema",
                "status": "failed",
                "error_kind": "provider_error",
                "message": err.to_string(),
            }));
            panic!("Gemini provider probe failed: {err}");
        }
    }
}

#[test]
fn provider_probe_parser_fixtures_cover_tool_argument_shapes() {
    let openai = parse_openai_response(
        r#"{"output":[{"type":"function_call","name":"Write","call_id":"c1","arguments":"{\"file\":\"provider-probe.txt\",\"body\":\"ok\"}"}]}"#,
    )
    .expect("OpenAI string arguments fixture");
    assert_eq!(openai.tool_calls[0].arguments["path"], "provider-probe.txt");
    assert_eq!(openai.tool_calls[0].arguments["content"], "ok");

    let gemini = parse_interactions_response(
        r#"{"output":[{"type":"function_call","name":"Write","call_id":"c1","arguments":"{\"file\":\"provider-probe.txt\",\"body\":\"ok\"}"}]}"#,
    )
    .expect("Gemini string arguments fixture");
    assert_eq!(gemini.tool_calls[0].arguments["path"], "provider-probe.txt");
    assert_eq!(gemini.tool_calls[0].arguments["content"], "ok");

    let ollama = parse_chat_response(
        r#"{"message":{"content":"<function_call>{\"name\":\"Write\",\"arguments\":{\"file\":\"provider-probe.txt\",\"body\":\"ok\"}}</function_call>"}}"#,
        &["Write".to_string()],
        true,
    )
    .expect("Ollama XML fallback fixture");
    assert_eq!(ollama.tool_calls[0].name, "Write");
    assert_eq!(ollama.tool_calls[0].arguments["path"], "provider-probe.txt");
    assert_eq!(ollama.tool_calls[0].arguments["content"], "ok");

    record_provider_probe(json!({
        "provider": "fixture",
        "probe": "tool_argument_parser_shapes",
        "status": "passed",
        "providers": ["openai", "gemini", "ollama_xml_fallback"],
    }));
}

#[test]
fn provider_probe_ollama_xml_fallback_tool_like_output() {
    if !provider_probe_enabled() {
        return;
    }
    let reply = parse_chat_response(
        r#"{"message":{"content":"<function_call>{\"name\":\"Write\",\"arguments\":{\"file\":\"provider-probe.txt\",\"body\":\"ok\"}}</function_call>"}}"#,
        &["Write".to_string()],
        true,
    )
    .expect("Ollama XML fallback probe fixture");
    let passed = reply.tool_calls.len() == 1
        && reply.tool_calls[0].name == "Write"
        && reply.tool_calls[0].arguments["path"] == "provider-probe.txt"
        && reply.tool_calls[0].arguments["content"] == "ok";
    record_provider_probe(json!({
        "provider": "ollama",
        "probe": "xml_fallback_tool_like_output",
        "status": if passed { "passed" } else { "failed" },
        "tool_calls": reply.tool_calls.len(),
        "tool_names": reply.tool_calls.iter().map(|call| call.name.clone()).collect::<Vec<_>>(),
        "arguments_shape": if passed { "object_recovered" } else { "unexpected" },
        "live": false,
    }));
    assert!(
        passed,
        "Ollama XML fallback did not recover tool-like output"
    );
}

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
        completion_contract_path: None,
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
        completion_contract_path: None,
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

fn provider_probe_enabled() -> bool {
    std::env::var("ANVIL_PROVIDER_PROBE").ok().as_deref() == Some("1")
        || std::env::var("ANVIL_LIVE_PROVIDER_TESTS").ok().as_deref() == Some("1")
}

fn record_provider_probe(mut value: Value) {
    let Some(path) = std::env::var_os("ANVIL_PROVIDER_PROBE_OUT").map(PathBuf::from) else {
        return;
    };
    let _guard = PROVIDER_PROBE_EVENT_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    value["event"] = json!("provider_probe");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{}", value);
    }
}

fn smoke_config(tmp_root: &Path, key_root: PathBuf, provider: Provider) -> Config {
    Config {
        workspace_root: key_root,
        state_dir: tmp_root.join("state"),
        eval_events_path: None,
        completion_contract_path: None,
        yes: true,
        offline: false,
        context_budget: 4096,
        model: "unused".to_string(),
        provider,
        planner_model: "unused".to_string(),
        planner_provider: provider,
        ollama_host: "http://127.0.0.1:11434".to_string(),
        num_predict: 512,
        max_iterations: 1,
        chat_timeout_secs: 60,
        chat_retries: 0,
        resume: None,
        fresh_session: true,
        no_footer: false,
        profile: "generic".to_string(),
        style: "default".to_string(),
        action: Action::Prompt(String::new()),
    }
}
