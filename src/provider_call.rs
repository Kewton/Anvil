use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::gemini::GeminiClient;
use crate::logging;
use crate::ollama::client::{AssistantReply, OllamaClient};
use crate::openai::OpenAiClient;
use crate::session::store::ConversationMessage;
use crate::tools::registry::ToolSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCallScope {
    PlannerUltra,
    PlannerStep,
    Executor,
    Repair,
}

impl ProviderCallScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlannerUltra => "planner_ultra",
            Self::PlannerStep => "planner_step",
            Self::Executor => "executor",
            Self::Repair => "repair",
        }
    }

    pub fn planner_timeout_kind(self) -> Option<&'static str> {
        match self {
            Self::PlannerUltra => Some("planner_ultra_timeout"),
            Self::PlannerStep => Some("phase_step_planner_timeout"),
            Self::Executor | Self::Repair => None,
        }
    }
}

pub(crate) fn run_bounded_provider_call<T, F>(
    scope: ProviderCallScope,
    timeout_secs: u64,
    call: F,
) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let timeout = Duration::from_secs(timeout_secs);
    let started = Instant::now();
    let (tx, rx) = mpsc::sync_channel(1);

    thread::spawn(move || {
        let result = call();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => {
            emit_provider_duration(
                scope,
                timeout_secs,
                started.elapsed(),
                if result.is_ok() { "ok" } else { "error" },
            );
            result.map_err(|err| classify_provider_error(scope, timeout_secs, err))
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            emit_provider_duration(
                scope,
                timeout_secs,
                started.elapsed(),
                "provider_turn_timeout",
            );
            Err(timeout_error(scope, timeout_secs))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            emit_provider_duration(
                scope,
                timeout_secs,
                started.elapsed(),
                "worker_disconnected",
            );
            Err("provider call worker disconnected".to_string())
        }
    }
}

pub(crate) fn chat_ollama_with_mode(
    scope: ProviderCallScope,
    client: &OllamaClient,
    model: &str,
    messages: &[ConversationMessage],
    tools: &[ToolSpec],
    native_tools_enabled: bool,
) -> Result<AssistantReply, String> {
    let client = client.clone();
    let model = model.to_string();
    let messages = messages.to_vec();
    let tools = tools.to_vec();
    let timeout_secs = client.timeout_secs();
    run_bounded_provider_call(scope, timeout_secs, move || {
        client.chat_with_mode(&model, &messages, &tools, native_tools_enabled)
    })
}

pub(crate) fn chat_ollama_streaming_with_mode<F>(
    scope: ProviderCallScope,
    client: &OllamaClient,
    model: &str,
    messages: &[ConversationMessage],
    tools: &[ToolSpec],
    native_tools_enabled: bool,
    on_chunk: F,
) -> Result<AssistantReply, String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    let started = Instant::now();
    let result =
        client.chat_streaming_with_mode(model, messages, tools, native_tools_enabled, on_chunk);
    emit_provider_duration(
        scope,
        client.timeout_secs(),
        started.elapsed(),
        if result.is_ok() { "ok" } else { "error" },
    );
    result.map_err(|err| classify_provider_error(scope, client.timeout_secs(), err))
}

pub(crate) fn chat_gemini(
    scope: ProviderCallScope,
    client: &GeminiClient,
    model: &str,
    messages: &[ConversationMessage],
    tools: &[ToolSpec],
) -> Result<AssistantReply, String> {
    let client = client.clone();
    let model = model.to_string();
    let messages = messages.to_vec();
    let tools = tools.to_vec();
    let timeout_secs = client.timeout_secs();
    run_bounded_provider_call(scope, timeout_secs, move || {
        client.chat(&model, &messages, &tools)
    })
}

pub(crate) fn chat_openai(
    scope: ProviderCallScope,
    client: &OpenAiClient,
    model: &str,
    messages: &[ConversationMessage],
    tools: &[ToolSpec],
) -> Result<AssistantReply, String> {
    let client = client.clone();
    let model = model.to_string();
    let messages = messages.to_vec();
    let tools = tools.to_vec();
    let timeout_secs = client.timeout_secs();
    run_bounded_provider_call(scope, timeout_secs, move || {
        client.chat(&model, &messages, &tools)
    })
}

pub(crate) fn is_scoped_planner_timeout(scope: ProviderCallScope, error: &str) -> bool {
    scope
        .planner_timeout_kind()
        .is_some_and(|kind| error.contains(kind))
}

fn classify_provider_error(scope: ProviderCallScope, timeout_secs: u64, error: String) -> String {
    if scope.planner_timeout_kind().is_some() && is_provider_turn_timeout(&error) {
        return format!("{} ({error})", timeout_error(scope, timeout_secs));
    }
    error
}

fn is_provider_turn_timeout(error: &str) -> bool {
    error.to_ascii_lowercase().contains("provider_turn_timeout")
}

fn timeout_error(scope: ProviderCallScope, timeout_secs: u64) -> String {
    match scope.planner_timeout_kind() {
        Some(kind) => format!("{kind}: provider call timed out after {timeout_secs}s"),
        None => format!("provider_turn_timeout: provider call timed out after {timeout_secs}s"),
    }
}

fn emit_provider_duration(
    scope: ProviderCallScope,
    timeout_secs: u64,
    elapsed: Duration,
    outcome: &'static str,
) {
    let payload = json!({
        "caller_scope": scope.as_str(),
        "timeout_secs": timeout_secs,
        "elapsed_ms": elapsed.as_millis() as u64,
        "outcome": outcome,
    });
    logging::log_llm_event("provider_turn_duration", payload.clone());
    logging::log_llm_event("provider.turn.elapsed", payload);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planner_step_worker_timeout_is_classified() {
        let result = run_bounded_provider_call(
            ProviderCallScope::PlannerStep,
            0,
            || -> Result<(), String> {
                thread::sleep(Duration::from_millis(50));
                Ok(())
            },
        );

        let err = result.unwrap_err();
        assert!(err.starts_with("phase_step_planner_timeout:"), "{err}");
    }

    #[test]
    fn planner_provider_transport_timeout_is_classified() {
        let err = classify_provider_error(
            ProviderCallScope::PlannerUltra,
            5,
            "provider_turn_timeout: Ollama generate API timed out".to_string(),
        );

        assert!(err.starts_with("planner_ultra_timeout:"), "{err}");
    }
}
